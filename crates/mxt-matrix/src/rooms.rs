//! The room list: the joined rooms, the facts mxt reads per room, and the
//! summaries the core receives.

use std::collections::{HashMap, HashSet};

use matrix_sdk::{
    Client, Room, RoomDisplayName, RoomState,
    deserialized_responses::SyncOrStrippedState,
    notification_settings::{NotificationSettings, RoomNotificationMode},
    ruma::{self, OwnedRoomId, events::space::parent::SpaceParentEventContent},
};
use mxt_core::{
    ids::{RoomId, Timestamp},
    msg::Msg,
    room::{
        BridgeInfo, NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts, network_name,
    },
};
use tokio::sync::{
    broadcast::{
        self,
        error::{RecvError, TryRecvError},
    },
    mpsc::UnboundedSender,
};
use tracing::{debug, info, warn};

use crate::{Error, bridges::bridge_info, sync::SyncedRooms};

/// What mxt reads from the SDK about one room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoomFacts {
    pub id: RoomId,
    pub display_name: String,
    pub is_direct: bool,
    /// Display name of the other member when the room has exactly one direct target.
    pub dm_partner_name: Option<String>,
    pub unread: UnreadCounts,
    pub mode: NotificationMode,
    pub latest: Option<Timestamp>,
    pub bridge: Option<BridgeInfo>,
    pub canonical_parent_name: Option<String>,
    /// One entry per joined parent space, `""` for unnamed ones.
    pub joined_parent_names: Vec<String>,
}

pub(crate) fn summary(facts: &RoomFacts) -> RoomSummary {
    let joined_parents: Vec<&str> = facts
        .joined_parent_names
        .iter()
        .map(String::as_str)
        .collect();
    let network = network_name(
        facts.bridge.as_ref(),
        facts.canonical_parent_name.as_deref(),
        &joined_parents,
    );
    let room = facts
        .dm_partner_name
        .as_ref()
        .filter(|_| facts.is_direct)
        .unwrap_or(&facts.display_name)
        .clone();
    RoomSummary {
        id: facts.id.clone(),
        name: RoomName { network, room },
        kind: if facts.is_direct {
            RoomKind::Direct
        } else {
            RoomKind::Group
        },
        unread: facts.unread,
        mode: facts.mode,
        latest_activity: facts.latest,
    }
}

/// Turns the joined rooms into `Msg::RoomUpserted` and `Msg::RoomRemoved`:
/// every room at the start, then the rooms of each sync response
/// (`responses`), room info updates (read receipts and unread counts change
/// through them), notification settings changes, and changes of parent
/// spaces.
pub(crate) async fn run(mut rooms: Rooms, mut responses: broadcast::Receiver<SyncedRooms>) {
    let mut settings_changes = rooms.settings.subscribe_to_changes();
    let mut room_updates = rooms.client.room_info_notable_update_receiver();
    // After subscribing, so that no change is missed.
    rooms.reload().await;
    loop {
        tokio::select! {
            response = responses.recv() => match response {
                Ok(updated) => rooms.check(updated.iter().cloned()).await,
                Err(RecvError::Lagged(missed)) => {
                    info!("missed the rooms of {missed} sync responses; re-reading every room");
                    rooms.reload().await;
                }
                Err(RecvError::Closed) => break,
            },
            change = settings_changes.recv() => match change {
                Ok(()) | Err(RecvError::Lagged(_)) => rooms.refresh_modes().await,
                Err(RecvError::Closed) => break,
            },
            updates = next_updates(&mut room_updates, |update| update.room_id) => match updates {
                Ok(updated) => rooms.updated(updated).await,
                Err(RecvError::Lagged(missed)) => {
                    info!("missed {missed} room info updates; re-reading every room");
                    rooms.reload().await;
                }
                Err(RecvError::Closed) => break,
            },
        }
    }
}

/// Waits for the next update, then returns its room and the rooms of the
/// updates queued behind it, each once: the SDK sends several updates per
/// room for one sync response. Cancel safe.
async fn next_updates<T: Clone>(
    updates: &mut broadcast::Receiver<T>,
    room_id: fn(T) -> OwnedRoomId,
) -> Result<HashSet<OwnedRoomId>, RecvError> {
    let mut rooms = HashSet::from([room_id(updates.recv().await?)]);
    loop {
        match updates.try_recv() {
            Ok(update) => {
                rooms.insert(room_id(update));
            }
            Err(TryRecvError::Lagged(missed)) => return Err(RecvError::Lagged(missed)),
            // The next `recv` reports a closed channel.
            Err(TryRecvError::Empty | TryRecvError::Closed) => return Ok(rooms),
        }
    }
}

/// Joined rooms that are not spaces, only the active version of an upgraded
/// room (see `active_version`).
fn is_listed(client: &Client, room: &Room) -> bool {
    room.state() == RoomState::Joined
        && !room.is_space()
        && active_version(
            room.successor_room()
                .and_then(|successor| client.get_room(&successor.room_id))
                .map(|successor| successor.state()),
        )
}

/// Whether a room is the active version of an upgraded room, from the state
/// of the room that replaced it (`None`: none did, or the client does not
/// know it). As in matrix-sdk-ui's room list, a successor the user has
/// joined, left or been banned from replaces the room; an invitation to it
/// or a knock on it does not.
fn active_version(successor: Option<RoomState>) -> bool {
    !matches!(
        successor,
        Some(RoomState::Joined | RoomState::Left | RoomState::Banned)
    )
}

/// The room list state of `run`.
pub(crate) struct Rooms {
    client: Client,
    settings: NotificationSettings,
    /// Send errors are ignored: a closed channel means the core is gone and
    /// mxt is exiting.
    to_core: UnboundedSender<Msg>,
    /// The rooms the core has, with their facts.
    listed: HashMap<OwnedRoomId, Listed>,
    /// The listed rooms per parent space (every `m.space.parent`, joined or not).
    spaces: Spaces,
}

struct Listed {
    facts: RoomFacts,
    parents: Vec<OwnedRoomId>,
}

impl Rooms {
    pub(crate) fn new(
        client: Client,
        settings: NotificationSettings,
        to_core: UnboundedSender<Msg>,
    ) -> Rooms {
        Rooms {
            client,
            settings,
            to_core,
            listed: HashMap::new(),
            spaces: Spaces::default(),
        }
    }

    /// Re-reads every joined room, and removes the listed rooms that are not
    /// listed any more.
    async fn reload(&mut self) {
        let mut rooms: Vec<OwnedRoomId> = self
            .client
            .joined_rooms()
            .iter()
            .map(|room| room.room_id().to_owned())
            .collect();
        rooms.extend(self.listed.keys().cloned());
        self.check(rooms).await;
    }

    /// After room info updates: the updated rooms, and the rooms that name an
    /// updated room as a parent space.
    async fn updated(&mut self, rooms: HashSet<OwnedRoomId>) {
        for room in &rooms {
            self.refresh_children(room).await;
        }
        self.check(rooms).await;
    }

    /// Upserts or removes each room of `updated` and the room it replaced,
    /// see `changes`.
    async fn check(&mut self, updated: impl IntoIterator<Item = OwnedRoomId>) {
        let (client, listed) = (&self.client, &self.listed);
        let changes = changes(
            updated,
            |room| Some(client.get_room(room)?.predecessor_room()?.room_id),
            |room| {
                client
                    .get_room(room)
                    .is_some_and(|room| is_listed(client, &room))
            },
            |room| listed.contains_key(room),
        );
        for room in changes.removed {
            self.forget(&room);
            debug!(%room, "room left the list");
            let _ = self
                .to_core
                .send(Msg::RoomRemoved(RoomId::new(room.as_str())));
        }
        for room in changes.upserted {
            if let Some(room) = self.client.get_room(&room) {
                self.refresh(&room).await;
            }
        }
    }

    /// Reads the room's facts and sends its summary when they changed.
    async fn refresh(&mut self, room: &Room) {
        let id = room.room_id().to_owned();
        let (facts, parents) = match room_facts(&self.client, &self.settings, room).await {
            Ok(facts) => facts,
            Err(error) => {
                warn!("reading a room for the room list failed: {}", error.brief());
                debug!(room = %id, "reading the room failed: {error}");
                return;
            }
        };
        if self
            .listed
            .get(&id)
            .is_none_or(|listed| listed.facts != facts)
        {
            let _ = self.to_core.send(Msg::RoomUpserted(summary(&facts)));
        }
        self.forget(&id);
        let parent_ids = parents.iter().map(|parent| parent.id.clone()).collect();
        for parent in parents {
            self.spaces.add_child(parent.id, parent.name, id.clone());
        }
        self.listed.insert(
            id,
            Listed {
                facts,
                parents: parent_ids,
            },
        );
    }

    fn forget(&mut self, room: &ruma::RoomId) {
        if let Some(listed) = self.listed.remove(room) {
            for parent in &listed.parents {
                self.spaces.remove_child(parent, room);
            }
        }
    }

    async fn refresh_modes(&mut self) {
        for (room, listed) in &mut self.listed {
            let mode = notification_mode(&self.settings, room).await;
            if listed.facts.mode != mode {
                listed.facts.mode = mode;
                let _ = self.to_core.send(Msg::RoomUpserted(summary(&listed.facts)));
            }
        }
    }

    /// After an update of `room`: the rooms that name it as a parent space,
    /// when what they read from it changed.
    async fn refresh_children(&mut self, room: &ruma::RoomId) {
        if !self.spaces.contains(room) {
            return;
        }
        let name = match joined_space_name(&self.client, room).await {
            Ok(name) => name,
            Err(error) => {
                warn!("reading a parent space failed: {}", error.brief());
                debug!(space = %room, "reading the space failed: {error}");
                return;
            }
        };
        for child in self.spaces.changed(room, name) {
            if let Some(child) = self.client.get_room(&child) {
                self.refresh(&child).await;
            }
        }
    }
}

/// The listed rooms per parent space, with what they read from the space.
#[derive(Default)]
struct Spaces(HashMap<OwnedRoomId, Space>);

struct Space {
    /// See `joined_space_name`.
    name: Option<String>,
    children: HashSet<OwnedRoomId>,
}

impl Spaces {
    /// `child` names `space` as a parent and read `name` from it.
    ///
    /// A known space keeps the name it had when it was added or last
    /// changed, which its other children were refreshed with. A child may
    /// read a newer name first, through an update of its own before the
    /// space's; `changed` then still sees the change and refreshes the rest.
    fn add_child(&mut self, space: OwnedRoomId, name: Option<String>, child: OwnedRoomId) {
        self.0
            .entry(space)
            .or_insert_with(|| Space {
                name,
                children: HashSet::new(),
            })
            .children
            .insert(child);
    }

    fn remove_child(&mut self, space: &ruma::RoomId, child: &ruma::RoomId) {
        if let Some(entry) = self.0.get_mut(space) {
            entry.children.remove(child);
            if entry.children.is_empty() {
                self.0.remove(space);
            }
        }
    }

    fn contains(&self, space: &ruma::RoomId) -> bool {
        self.0.contains_key(space)
    }

    /// After an update of `space`, which now reads `name`: its children to
    /// refresh, none when the name did not change.
    fn changed(&mut self, space: &ruma::RoomId, name: Option<String>) -> Vec<OwnedRoomId> {
        match self.0.get_mut(space) {
            Some(entry) if entry.name != name => {
                entry.name = name;
                entry.children.iter().cloned().collect()
            }
            _ => Vec::new(),
        }
    }
}

/// What re-reading some rooms changes in the list.
#[derive(Debug, Default, PartialEq, Eq)]
struct Changes {
    /// Rooms to read, and to send when their facts changed.
    upserted: Vec<OwnedRoomId>,
    /// Listed rooms that are not listed any more.
    removed: Vec<OwnedRoomId>,
}

/// The rooms of `updated` (the joined and left rooms of a sync response, or
/// the rooms of room info updates), each once, and the room each of them
/// replaced, because whether an upgraded room is listed depends on its
/// successor (see `is_listed`). Every room that is `listable` is upserted;
/// the others are removed when they are `listed`.
fn changes(
    updated: impl IntoIterator<Item = OwnedRoomId>,
    predecessor: impl Fn(&ruma::RoomId) -> Option<OwnedRoomId>,
    listable: impl Fn(&ruma::RoomId) -> bool,
    listed: impl Fn(&ruma::RoomId) -> bool,
) -> Changes {
    let mut seen = HashSet::new();
    let mut changes = Changes::default();
    for room in updated {
        let replaced = predecessor(&room);
        for room in std::iter::once(room).chain(replaced) {
            if !seen.insert(room.clone()) {
                continue;
            }
            if listable(&room) {
                changes.upserted.push(room);
            } else if listed(&room) {
                changes.removed.push(room);
            }
        }
    }
    changes
}

async fn room_facts(
    client: &Client,
    settings: &NotificationSettings,
    room: &Room,
) -> Result<(RoomFacts, Vec<Parent>), Error> {
    let is_direct = room.is_direct().await?;
    let dm_partner_name = if is_direct {
        dm_partner_name(room).await?
    } else {
        None
    };
    let parents = parent_spaces(client, room).await?;
    let facts = RoomFacts {
        id: RoomId::new(room.room_id().as_str()),
        display_name: room.display_name().await?.to_string(),
        is_direct,
        dm_partner_name,
        unread: UnreadCounts {
            messages: room.num_unread_messages(),
            notifications: room.num_unread_notifications(),
            mentions: room.num_unread_mentions(),
            marked: room.is_marked_unread(),
        },
        mode: notification_mode(settings, room.room_id()).await,
        latest: room
            .latest_event_timestamp()
            .map(|ts| Timestamp::from_millis(ts.0.into())),
        bridge: bridge_info(room).await?,
        canonical_parent_name: parents
            .iter()
            .find(|parent| parent.canonical && parent.name.is_some())
            .and_then(|parent| parent.name.clone()),
        joined_parent_names: parents
            .iter()
            .filter_map(|parent| parent.name.clone())
            .collect(),
    };
    Ok((facts, parents))
}

/// From the store only: no `/members` request.
async fn dm_partner_name(room: &Room) -> Result<Option<String>, Error> {
    let targets = room.direct_targets();
    let mut targets = targets.iter();
    let (Some(target), None) = (targets.next(), targets.next()) else {
        return Ok(None);
    };
    let Some(user) = target.as_user_id() else {
        return Ok(None);
    };
    let member = room.get_member_no_sync(user).await?;
    Ok(member
        .and_then(|member| member.display_name().map(str::to_owned))
        .filter(|name| !name.trim().is_empty()))
}

/// The room's notification rule. Without one (`None` from the SDK) the
/// account default applies, which is `Default`.
async fn notification_mode(
    settings: &NotificationSettings,
    room: &ruma::RoomId,
) -> NotificationMode {
    match settings.get_user_defined_room_notification_mode(room).await {
        None | Some(RoomNotificationMode::AllMessages) => NotificationMode::Default,
        Some(RoomNotificationMode::MentionsAndKeywordsOnly) => {
            NotificationMode::MentionsAndKeywords
        }
        Some(RoomNotificationMode::Mute) => NotificationMode::Mute,
    }
}

/// One `m.space.parent` of a room.
struct Parent {
    id: OwnedRoomId,
    canonical: bool,
    /// See `joined_space_name`.
    name: Option<String>,
}

/// From the room's `m.space.parent` state, joined spaces or not.
async fn parent_spaces(client: &Client, room: &Room) -> Result<Vec<Parent>, Error> {
    let mut parents = Vec::new();
    for raw in room
        .get_state_events_static::<SpaceParentEventContent>()
        .await?
    {
        let (id, canonical) = match raw.deserialize() {
            Ok(SyncOrStrippedState::Sync(event)) => match event.as_original() {
                Some(event) => (event.state_key.clone(), event.content.canonical),
                // Redacted.
                None => continue,
            },
            Ok(SyncOrStrippedState::Stripped(event)) => {
                (event.state_key.clone(), event.content.canonical)
            }
            Err(_) => continue,
        };
        let name = joined_space_name(client, &id).await?;
        parents.push(Parent {
            id,
            canonical,
            name,
        });
    }
    Ok(parents)
}

/// The name of a space the user has joined, `None` for any other room. `""`
/// for a space without a name or alias: a name the SDK calculates from
/// members is no network name.
async fn joined_space_name(client: &Client, space: &ruma::RoomId) -> Result<Option<String>, Error> {
    let Some(space) = client
        .get_room(space)
        .filter(|space| space.state() == RoomState::Joined && space.is_space())
    else {
        return Ok(None);
    };
    Ok(Some(match space.display_name().await? {
        RoomDisplayName::Named(name) | RoomDisplayName::Aliased(name) => name,
        _ => String::new(),
    }))
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::owned_room_id;

    use super::*;

    fn facts(display_name: &str) -> RoomFacts {
        RoomFacts {
            id: RoomId::new("!room:example.org"),
            display_name: display_name.to_owned(),
            is_direct: false,
            dm_partner_name: None,
            unread: UnreadCounts::default(),
            mode: NotificationMode::Default,
            latest: None,
            bridge: None,
            canonical_parent_name: None,
            joined_parent_names: Vec::new(),
        }
    }

    fn bridge(network: Option<&str>, protocol: Option<&str>) -> Option<BridgeInfo> {
        Some(BridgeInfo {
            network: network.map(Into::into),
            protocol: protocol.map(Into::into),
        })
    }

    fn name(network: Option<&str>, room: &str) -> RoomName {
        RoomName {
            network: network.map(Into::into),
            room: room.to_owned(),
        }
    }

    #[test]
    fn bridged_channel_takes_the_network_from_the_bridge() {
        let channel = RoomFacts {
            bridge: bridge(Some("myserver"), Some("Discord")),
            joined_parent_names: vec!["myserver space".into()],
            ..facts("general")
        };
        let summary = summary(&channel);
        assert_eq!(summary.name, name(Some("myserver"), "general"));
        assert_eq!(summary.kind, RoomKind::Group);
    }

    #[test]
    fn bridged_direct_message_uses_the_protocol_and_the_partner_name() {
        let dm = RoomFacts {
            is_direct: true,
            dm_partner_name: Some("Alice".into()),
            bridge: bridge(None, Some("Discord")),
            ..facts("Alice (Discord)")
        };
        let summary = summary(&dm);
        assert_eq!(summary.name, name(Some("Discord"), "Alice"));
        assert_eq!(summary.name.to_string(), "Discord / Alice");
        assert_eq!(summary.kind, RoomKind::Direct);
    }

    #[test]
    fn direct_message_without_a_partner_name_uses_the_display_name() {
        let dm = RoomFacts {
            is_direct: true,
            ..facts("Bob")
        };
        let summary = summary(&dm);
        assert_eq!(summary.name, name(None, "Bob"));
        assert_eq!(summary.kind, RoomKind::Direct);
    }

    #[test]
    fn room_is_named_through_its_canonical_parent_space() {
        let room = RoomFacts {
            canonical_parent_name: Some("Book club".into()),
            joined_parent_names: vec!["Book club".into(), "Readers".into()],
            ..facts("Chapter 1")
        };
        assert_eq!(summary(&room).name, name(Some("Book club"), "Chapter 1"));
    }

    #[test]
    fn room_with_two_joined_parents_and_no_bridge_has_no_network() {
        let room = RoomFacts {
            joined_parent_names: vec!["One".into(), "Two".into()],
            ..facts("Lobby")
        };
        assert_eq!(summary(&room).name, name(None, "Lobby"));
    }

    #[test]
    fn counts_mode_and_latest_activity_pass_through() {
        let unread = UnreadCounts {
            messages: 9,
            notifications: 4,
            mentions: 2,
            marked: true,
        };
        let room = RoomFacts {
            unread,
            mode: NotificationMode::Mute,
            latest: Some(Timestamp::from_millis(1_700_000_000_000)),
            ..facts("Lobby")
        };
        let summary = summary(&room);
        assert_eq!(summary.id, RoomId::new("!room:example.org"));
        assert_eq!(summary.unread, unread);
        assert_eq!(summary.mode, NotificationMode::Mute);
        assert_eq!(
            summary.latest_activity,
            Some(Timestamp::from_millis(1_700_000_000_000))
        );
    }

    #[test]
    fn updated_rooms_are_listed_or_removed_with_the_rooms_they_replaced() {
        let (kept, joined, left, space, stranger, old, new) = (
            owned_room_id!("!kept:example.org"),
            owned_room_id!("!joined:example.org"),
            owned_room_id!("!left:example.org"),
            owned_room_id!("!space:example.org"),
            owned_room_id!("!stranger:example.org"),
            owned_room_id!("!old:example.org"),
            owned_room_id!("!new:example.org"),
        );
        let listed = HashSet::from([kept.clone(), left.clone(), old.clone()]);
        // After the response: the user joined `joined` and `new`, which
        // replaces `old`, and left `left` and `stranger`, which was never
        // listed; `space` is a space.
        let listable = HashSet::from([kept.clone(), joined.clone(), new.clone()]);
        let predecessors = HashMap::from([(new.clone(), old.clone())]);
        let changes = changes(
            [
                joined.clone(),
                kept.clone(),
                left.clone(),
                space,
                new.clone(),
                joined.clone(),
                stranger,
            ],
            |room| predecessors.get(room).cloned(),
            |room| listable.contains(room),
            |room| listed.contains(room),
        );
        assert_eq!(
            changes,
            Changes {
                upserted: vec![joined, kept, new],
                removed: vec![left, old],
            }
        );
    }

    #[test]
    fn only_a_successor_the_user_joined_left_or_was_banned_from_hides_a_room() {
        assert!(active_version(None));
        for kept in [RoomState::Invited, RoomState::Knocked] {
            assert!(active_version(Some(kept)), "{kept:?}");
        }
        for replaced in [RoomState::Joined, RoomState::Left, RoomState::Banned] {
            assert!(!active_version(Some(replaced)), "{replaced:?}");
        }
    }

    #[test]
    fn a_space_update_refreshes_its_children_only_when_its_name_changed() {
        let (space, a, b) = (
            owned_room_id!("!space:example.org"),
            owned_room_id!("!a:example.org"),
            owned_room_id!("!b:example.org"),
        );
        let club = || Some("Book club".to_owned());
        let readers = || Some("Readers".to_owned());
        let sorted = |mut rooms: Vec<OwnedRoomId>| {
            rooms.sort();
            rooms
        };
        let mut spaces = Spaces::default();
        spaces.add_child(space.clone(), club(), a.clone());
        spaces.add_child(space.clone(), club(), b.clone());
        assert!(spaces.changed(&space, club()).is_empty());

        // `a` reads the new name through its own update before the space's
        // (`Rooms::refresh` forgets the room, then adds it again).
        spaces.remove_child(&space, &a);
        spaces.add_child(space.clone(), readers(), a.clone());
        assert_eq!(
            sorted(spaces.changed(&space, readers())),
            vec![a.clone(), b.clone()]
        );
        assert!(spaces.changed(&space, readers()).is_empty());
        // The user left the space.
        assert_eq!(
            sorted(spaces.changed(&space, None)),
            vec![a.clone(), b.clone()]
        );

        spaces.remove_child(&space, &a);
        assert!(spaces.contains(&space));
        spaces.remove_child(&space, &b);
        assert!(!spaces.contains(&space));
    }
}

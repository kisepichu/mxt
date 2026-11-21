//! The room list: the joined rooms, the facts mxt reads per room, the
//! summaries the core receives, and the pending invites.

use std::collections::{HashMap, HashSet};

use matrix_sdk::{
    Client, Room, RoomDisplayName, RoomMemberships, RoomState,
    deserialized_responses::SyncOrStrippedState,
    notification_settings::{NotificationSettings, RoomNotificationMode},
    ruma::{
        self, OwnedRoomId, OwnedUserId,
        events::{
            receipt::{ReceiptThread, ReceiptType},
            space::parent::SpaceParentEventContent,
        },
    },
};
use mxt_core::{
    ids::{RoomId, Timestamp},
    msg::Msg,
    room::{
        BridgeInfo, Invite, NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts,
        network_name,
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

use crate::{Error, bridges, sync::SyncedRooms};

/// What mxt reads from the SDK about one room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoomFacts {
    pub id: RoomId,
    pub display_name: String,
    pub is_direct: bool,
    /// Display name of the other person of a direct message: the one direct
    /// target, or the one joined member besides the user and the bridge bot.
    pub dm_partner_name: Option<String>,
    pub unread: UnreadCounts,
    pub mode: NotificationMode,
    pub latest: Option<Timestamp>,
    /// The time of the user's own latest read receipt (see `last_read`).
    pub last_read: Option<Timestamp>,
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
        last_read: facts.last_read,
    }
}

/// Turns the joined rooms into `Msg::RoomUpserted` and `Msg::RoomRemoved`:
/// every room at the start, then the rooms of each sync response
/// (`responses`), room info updates (read receipts and unread counts change
/// through them), notification settings changes, and changes of parent
/// spaces. The pending invites go to the core as `Msg::Invites` at the
/// start and when a sync response changed them.
pub(crate) async fn run(mut rooms: Rooms, mut responses: broadcast::Receiver<SyncedRooms>) {
    let mut settings_changes = rooms.settings.subscribe_to_changes();
    let mut room_updates = rooms.client.room_info_notable_update_receiver();
    // After subscribing, so that no change is missed.
    rooms.reload().await;
    rooms.check_invites().await;
    loop {
        tokio::select! {
            response = responses.recv() => match response {
                Ok(updated) => {
                    rooms.check(updated.iter().cloned()).await;
                    rooms.check_invites().await;
                }
                Err(RecvError::Lagged(missed)) => {
                    info!("missed the rooms of {missed} sync responses; re-reading every room");
                    rooms.reload().await;
                    rooms.check_invites().await;
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
    /// The pending invites the core has, sorted by name; `None` before the
    /// first `Msg::Invites`.
    invites: Option<Vec<Invite>>,
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
            invites: None,
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

    /// Sends the invited rooms' IDs and display names as `Msg::Invites`,
    /// sorted by name, when they differ from the ones sent last. An invite's
    /// name often resolves a sync after the invite arrives, so names count.
    pub(crate) async fn check_invites(&mut self) {
        let mut invites = Vec::new();
        for room in self.client.invited_rooms() {
            match room.display_name().await {
                Ok(name) => invites.push(Invite {
                    room: RoomId::new(room.room_id().as_str()),
                    name: name.to_string(),
                }),
                Err(error) => {
                    let error = Error::from(error);
                    warn!("reading a pending invite failed: {}", error.brief());
                    debug!(room = %room.room_id(), "reading the invite failed: {error}");
                    return;
                }
            }
        }
        invites.sort_by_cached_key(|invite| {
            (
                invite.name.to_lowercase(),
                invite.name.clone(),
                invite.room.clone(),
            )
        });
        if self.invites.as_ref() != Some(&invites) {
            let _ = self.to_core.send(Msg::Invites(invites.clone()));
            self.invites = Some(invites);
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

/// The rooms of `updated` (the joined, left and invited rooms of a sync
/// response, or the rooms of room info updates), each once, and the room
/// each of them replaced, because whether an upgraded room is listed
/// depends on its successor (see `is_listed`). Every room that is
/// `listable` is upserted; the others are removed when they are `listed`.
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
    let bridge = bridges::bridge(room).await?;
    let bridged_partner = match &bridge {
        Some(bridge) => bridged_dm_partner(room, bridge)
            .await?
            .map(|partner| partner.name),
        None => None,
    };
    let (is_direct, dm_partner_name) = match bridged_partner {
        Some(name) => (true, Some(name)),
        None if room.is_direct().await? => (true, dm_partner_name(room).await?),
        None => (false, None),
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
        last_read: last_read(room).await?,
        bridge: bridge.map(|bridge| bridge.info),
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

/// The time of the user's own latest read receipt in the room, from any
/// device (design, "Room list"): the latest `ts` among the stored `m.read`
/// and `m.read.private` receipts, unthreaded and on the main timeline.
/// Receipts in threads do not count: the store keeps them per thread root.
/// `None` when the store has none with a time.
async fn last_read(room: &Room) -> Result<Option<Timestamp>, Error> {
    let mut latest = None;
    for receipt_type in [ReceiptType::Read, ReceiptType::ReadPrivate] {
        for thread in [ReceiptThread::Unthreaded, ReceiptThread::Main] {
            let receipt = room
                .load_user_receipt(receipt_type.clone(), &thread, room.own_user_id())
                .await?;
            latest = latest.max(receipt.and_then(|(_, receipt)| receipt.ts));
        }
    }
    Ok(latest.map(|ts| Timestamp::from_millis(ts.0.into())))
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

/// The other person of a bridged direct message.
pub(crate) struct Partner {
    pub id: OwnedUserId,
    /// The display name, or the user ID without one.
    pub name: String,
}

/// The other person of a bridged direct message (see `DmCandidate`), from
/// the store only: no `/members` request. The room list names the room
/// after them, and `:join` finds the room by their user ID.
pub(crate) async fn bridged_dm_partner(
    room: &Room,
    bridge: &bridges::Bridge,
) -> Result<Option<Partner>, Error> {
    let Some(bot) = bridge.bot.as_deref() else {
        return Ok(None);
    };
    // The SDK counts an empty `m.room.name` as none.
    let named = room.name().is_some();
    let joined_count = room.joined_members_count();
    // Other rooms' members are not read.
    let Some(candidate) = DmCandidate::new(bot, bridge.room_type, named, joined_count) else {
        return Ok(None);
    };
    let members = room.members_no_sync(RoomMemberships::JOIN).await?;
    let joined: Vec<JoinedMember<'_>> = members
        .iter()
        .map(|member| JoinedMember {
            id: member.user_id(),
            display_name: member.display_name(),
        })
        .collect();
    Ok(candidate
        .partner(room.own_user_id(), &joined)
        .map(|person| Partner {
            id: person.id.to_owned(),
            name: person.name(),
        }))
}

/// A joined member as the store knows it.
struct JoinedMember<'a> {
    id: &'a ruma::UserId,
    display_name: Option<&'a str>,
}

impl JoinedMember<'_> {
    /// The display name, or the user ID without one.
    fn name(&self) -> String {
        self.display_name
            .filter(|name| !name.trim().is_empty())
            .map_or_else(|| self.id.to_string(), str::to_owned)
    }
}

/// A bridged room that may be a direct message (phase 0 finding 2), before
/// its members are read. Bridges such as mautrix-discord keep their bot (from
/// the bridge state) in the direct messages they bridge, so the room has two
/// other members and the SDK names it after both.
struct DmCandidate<'a> {
    bot: &'a ruma::UserId,
    joined_count: u64,
}

impl<'a> DmCandidate<'a> {
    /// A room the bridge declares a direct message (`dm`), or one it declares
    /// no type for and that has no `m.room.name`, with the user and one
    /// person besides the bot (`joined_count`, from the server). mautrix
    /// bridges declare no type for channels either, but name them after the
    /// channel, so a named channel where only one person has spoken is not
    /// taken for a direct message. Group direct messages, spaces and other
    /// declared types never are.
    fn new(
        bot: &'a ruma::UserId,
        room_type: Option<bridges::RoomType>,
        named: bool,
        joined_count: u64,
    ) -> Option<DmCandidate<'a>> {
        let may_be_dm = match room_type {
            Some(bridges::RoomType::Dm) => true,
            Some(bridges::RoomType::GroupDm | bridges::RoomType::Other) => false,
            None => !named,
        };
        (may_be_dm && (2..=3).contains(&joined_count)).then_some(DmCandidate { bot, joined_count })
    }

    /// The person the room is a direct message with: the only joined member
    /// besides the user (`me`) and the bot. Members are lazy-loaded, so
    /// `joined` (from the store) may lack members that the server counts:
    /// the person must be the only joined member the count leaves besides
    /// the user and the bot.
    fn partner<'m, 'j>(
        &self,
        me: &ruma::UserId,
        joined: &'m [JoinedMember<'j>],
    ) -> Option<&'m JoinedMember<'j>> {
        let bot_joined = joined.iter().any(|member| member.id == self.bot);
        if self.joined_count.checked_sub(1 + u64::from(bot_joined)) != Some(1) {
            return None;
        }
        let mut others = joined
            .iter()
            .filter(|member| member.id != me && member.id != self.bot);
        let (Some(person), None) = (others.next(), others.next()) else {
            return None;
        };
        Some(person)
    }
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
    use matrix_sdk::{
        ruma::{
            MilliSecondsSinceUnixEpoch, event_id, owned_device_id, owned_event_id, owned_room_id,
            room_id, user_id,
        },
        test_utils::mocks::MatrixMockServer,
    };
    use matrix_sdk_test::{JoinedRoomBuilder, event_factory::EventFactory};

    use super::*;
    use crate::bridges::RoomType;

    fn facts(display_name: &str) -> RoomFacts {
        RoomFacts {
            id: RoomId::new("!room:example.org"),
            display_name: display_name.to_owned(),
            is_direct: false,
            dm_partner_name: None,
            unread: UnreadCounts::default(),
            mode: NotificationMode::Default,
            latest: None,
            last_read: None,
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

    fn ids() -> [&'static ruma::UserId; 4] {
        [
            user_id!("@me:example.org"),
            user_id!("@discordbot:example.org"),
            user_id!("@discord_1:example.org"),
            user_id!("@discord_2:example.org"),
        ]
    }

    fn member<'a>(id: &'a ruma::UserId, display_name: Option<&'a str>) -> JoinedMember<'a> {
        JoinedMember { id, display_name }
    }

    /// The name of the partner `bridged_dm_partner` finds in a bridged room
    /// whose bridge declares `room_type`, with an `m.room.name` or without
    /// one.
    fn partner(
        room_type: Option<RoomType>,
        named: bool,
        joined_count: u64,
        joined: &[JoinedMember<'_>],
    ) -> Option<String> {
        let [me, bot, ..] = ids();
        let person = DmCandidate::new(bot, room_type, named, joined_count)?.partner(me, joined)?;
        Some(person.name())
    }

    /// The user, the bridge bot and Alice.
    fn with_the_bot_and_alice() -> [JoinedMember<'static>; 3] {
        let [me, bot, alice, _] = ids();
        [
            member(me, Some("Me")),
            member(bot, Some("Discord bridge bot")),
            member(alice, Some("Alice")),
        ]
    }

    #[test]
    fn an_unnamed_bridged_room_with_the_bot_and_one_person_is_a_direct_message() {
        let joined = with_the_bot_and_alice();
        assert_eq!(partner(None, false, 3, &joined), Some("Alice".to_owned()));
        // `:join` finds the room by the person's user ID.
        let [me, bot, alice, _] = ids();
        let candidate = DmCandidate::new(bot, None, false, 3).unwrap();
        assert_eq!(
            candidate.partner(me, &joined).map(|person| person.id),
            Some(alice)
        );
        // The user's own membership is not needed.
        assert_eq!(
            partner(None, false, 3, &joined[1..]),
            Some("Alice".to_owned())
        );
        // Without a display name, the user ID.
        let [me, bot, alice, _] = ids();
        let joined = [
            member(me, None),
            member(bot, None),
            member(alice, Some(" ")),
        ];
        assert_eq!(
            partner(None, false, 3, &joined),
            Some("@discord_1:example.org".to_owned())
        );
    }

    #[test]
    fn a_named_bridged_room_with_the_bot_and_one_person_is_no_direct_message() {
        // A mautrix-discord guild channel where only Alice has spoken: the
        // bridge declares no room type and names it after the channel.
        assert_eq!(partner(None, true, 3, &with_the_bot_and_alice()), None);
    }

    #[test]
    fn a_dm_marker_makes_a_bridged_room_a_direct_message_even_with_a_name() {
        let joined = with_the_bot_and_alice();
        assert_eq!(
            partner(Some(RoomType::Dm), true, 3, &joined),
            Some("Alice".to_owned())
        );
        assert_eq!(
            partner(Some(RoomType::Dm), false, 3, &joined),
            Some("Alice".to_owned())
        );
    }

    #[test]
    fn group_dms_and_other_declared_types_are_no_direct_messages() {
        let joined = with_the_bot_and_alice();
        for room_type in [RoomType::GroupDm, RoomType::Other] {
            assert_eq!(partner(Some(room_type), false, 3, &joined), None);
        }
    }

    #[test]
    fn a_bridged_room_with_one_person_and_no_bot_is_a_direct_message() {
        let [me, _, alice, _] = ids();
        let joined = [member(me, Some("Me")), member(alice, Some("Alice"))];
        assert_eq!(partner(None, false, 2, &joined), Some("Alice".to_owned()));
    }

    #[test]
    fn a_bridged_room_with_two_people_besides_the_bot_is_no_direct_message() {
        let [me, bot, alice, bob] = ids();
        let joined = [
            member(me, Some("Me")),
            member(bot, Some("Discord bridge bot")),
            member(alice, Some("Alice")),
            member(bob, Some("Bob")),
        ];
        assert_eq!(partner(Some(RoomType::Dm), false, 4, &joined), None);
        // Bob's membership is not loaded (lazy-loaded members), but the
        // server counts him.
        assert_eq!(partner(Some(RoomType::Dm), false, 4, &joined[..3]), None);
        // The bot's membership is not loaded, so the third member may be
        // the bot or another person.
        let joined = [member(me, Some("Me")), member(alice, Some("Alice"))];
        assert_eq!(partner(Some(RoomType::Dm), false, 3, &joined), None);
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
    fn counts_mode_and_times_pass_through() {
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
            last_read: Some(Timestamp::from_millis(1_600_000_000_000)),
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
        assert_eq!(
            summary.last_read,
            Some(Timestamp::from_millis(1_600_000_000_000))
        );
    }

    /// The user's latest `m.read` or `m.read.private` receipt, unthreaded or
    /// on the main timeline, from the store; not one in a thread, not
    /// another user's.
    #[tokio::test]
    async fn the_read_time_is_the_latest_of_the_users_receipts_outside_threads() {
        let server = MatrixMockServer::new().await;
        let me = user_id!("@alice:example.org");
        let client = server
            .client_builder()
            .logged_in_with_token("1234".to_owned(), me.to_owned(), owned_device_id!("DEVICE"))
            // Plain HTTP; see the adapter's tests.
            .on_builder(|builder| builder.disable_ssl_verification())
            .build()
            .await;
        let settings = client.notification_settings().await;
        let at = |millis: u32| Some(MilliSecondsSinceUnixEpoch(millis.into()));
        let receipts = EventFactory::new()
            .read_receipts()
            .add_with_timestamp(
                event_id!("$1:example.org"),
                me,
                ReceiptType::Read,
                ReceiptThread::Unthreaded,
                at(1000),
            )
            .add_with_timestamp(
                event_id!("$2:example.org"),
                me,
                ReceiptType::ReadPrivate,
                ReceiptThread::Main,
                at(2000),
            )
            .add_with_timestamp(
                event_id!("$3:example.org"),
                me,
                ReceiptType::Read,
                ReceiptThread::Thread(owned_event_id!("$root:example.org")),
                at(3000),
            )
            .add_with_timestamp(
                event_id!("$4:example.org"),
                user_id!("@bob:example.org"),
                ReceiptType::Read,
                ReceiptThread::Unthreaded,
                at(4000),
            )
            .into_event();
        let read = JoinedRoomBuilder::new(room_id!("!read:example.org")).add_receipt(receipts);
        let read = server.sync_room(&client, read).await;
        let (facts, _) = room_facts(&client, &settings, &read).await.unwrap();
        assert_eq!(facts.last_read, Some(Timestamp::from_millis(2000)));

        // The unthreaded `m.read`, which mxt sends by default and most
        // clients send, as the latest.
        let receipts = EventFactory::new()
            .read_receipts()
            .add_with_timestamp(
                event_id!("$5:example.org"),
                me,
                ReceiptType::Read,
                ReceiptThread::Unthreaded,
                at(2000),
            )
            .add_with_timestamp(
                event_id!("$6:example.org"),
                me,
                ReceiptType::ReadPrivate,
                ReceiptThread::Main,
                at(1000),
            )
            .into_event();
        let public = JoinedRoomBuilder::new(room_id!("!public:example.org")).add_receipt(receipts);
        let public = server.sync_room(&client, public).await;
        let (facts, _) = room_facts(&client, &settings, &public).await.unwrap();
        assert_eq!(facts.last_read, Some(Timestamp::from_millis(2000)));

        let never = JoinedRoomBuilder::new(room_id!("!never:example.org"));
        let never = server.sync_room(&client, never).await;
        let (facts, _) = room_facts(&client, &settings, &never).await.unwrap();
        assert_eq!(facts.last_read, None);
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

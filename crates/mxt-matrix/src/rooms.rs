//! The room list: the SDK's room stream, the facts mxt reads per room, and
//! the summaries the core receives.

use std::{
    collections::{HashMap, HashSet},
    pin::pin,
};

use futures_util::StreamExt as _;
use matrix_sdk::{
    Client, Room, RoomDisplayName, RoomState,
    deserialized_responses::SyncOrStrippedState,
    notification_settings::{NotificationSettings, RoomNotificationMode},
    ruma::{self, OwnedRoomId, events::space::parent::SpaceParentEventContent},
};
use matrix_sdk_ui::{
    eyeball_im::{Vector, VectorDiff},
    room_list_service::{
        RoomList, RoomListItem,
        filters::{
            BoxedFilterFn, new_filter_all, new_filter_deduplicate_versions, new_filter_joined,
            new_filter_not, new_filter_space,
        },
    },
};
use mxt_core::{
    ids::{RoomId, Timestamp},
    msg::Msg,
    room::{
        BridgeInfo, NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts, network_name,
    },
};
use tokio::sync::{
    broadcast::error::RecvError,
    mpsc::{UnboundedReceiver, UnboundedSender},
};
use tracing::{debug, info, warn};

use crate::{Error, bridges::BridgeStates};

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

/// Turns the SDK's room list into `Msg::RoomUpserted` and `Msg::RoomRemoved`:
/// the room stream's diffs, notification settings changes, changes of parent
/// spaces, and bridge state from `bridges`.
pub(crate) async fn run(
    room_list: RoomList,
    mut rooms: Rooms,
    mut bridges: UnboundedReceiver<BridgeStates>,
) {
    // One page that holds every room: the SDK's paging only limits what a
    // screen shows.
    let (stream, controller) = room_list.entries_with_dynamic_adapters(usize::MAX);
    controller.set_filter(listed_rooms_filter());
    let mut stream = pin!(stream);
    let mut settings_changes = rooms.settings.subscribe_to_changes();
    let mut room_updates = rooms.client.room_info_notable_update_receiver();
    let mut bridges_open = true;
    loop {
        tokio::select! {
            batch = stream.next() => match batch {
                Some(batch) => rooms.apply(batch).await,
                None => break,
            },
            change = settings_changes.recv() => match change {
                Ok(()) | Err(RecvError::Lagged(_)) => rooms.refresh_modes().await,
                Err(RecvError::Closed) => break,
            },
            // Listed rooms reach the stream as `Set` diffs; here only their
            // parent spaces matter.
            update = room_updates.recv() => match update {
                Ok(update) => rooms.refresh_children(&update.room_id).await,
                // The stream's own receiver of these updates is drained in
                // this task too, so it has most likely lagged as well, and it
                // drops what it missed: listed rooms would keep stale facts
                // and the SDK's filter stale room states. Setting the filter
                // again rebuilds the list from fresh rooms and yields a
                // `Reset`, which re-reads every listed room.
                Err(RecvError::Lagged(missed)) => {
                    info!("missed {missed} room info updates; rebuilding the room list");
                    controller.set_filter(listed_rooms_filter());
                }
                Err(RecvError::Closed) => break,
            },
            states = bridges.recv(), if bridges_open => match states {
                Some(states) => rooms.apply_bridges(states),
                None => bridges_open = false,
            },
        }
    }
}

/// Joined rooms that are not spaces, only the active version of an upgraded
/// room.
fn listed_rooms_filter() -> BoxedFilterFn {
    Box::new(new_filter_all(vec![
        Box::new(new_filter_joined()),
        Box::new(new_filter_not(Box::new(new_filter_space()))),
        Box::new(new_filter_deduplicate_versions()),
    ]))
}

/// The room list state of `run`.
pub(crate) struct Rooms {
    client: Client,
    settings: NotificationSettings,
    /// Send errors are ignored: a closed channel means the core is gone and
    /// mxt is exiting.
    to_core: UnboundedSender<Msg>,
    /// Rooms that appear in the list, for their bridge state.
    to_bridges: UnboundedSender<Vec<OwnedRoomId>>,
    /// The listed room IDs in the stream's order, to apply its diffs.
    order: Vector<OwnedRoomId>,
    /// The rooms the core has, with their facts.
    listed: HashMap<OwnedRoomId, Listed>,
    /// The listed rooms per parent space (every `m.space.parent`, joined or not).
    spaces: Spaces,
    /// Bridge state per room as far as known; an inner `None` means no bridge.
    bridges: HashMap<OwnedRoomId, Option<BridgeInfo>>,
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
        to_bridges: UnboundedSender<Vec<OwnedRoomId>>,
    ) -> Rooms {
        Rooms {
            client,
            settings,
            to_core,
            to_bridges,
            order: Vector::new(),
            listed: HashMap::new(),
            spaces: Spaces::default(),
            bridges: HashMap::new(),
        }
    }

    async fn apply(&mut self, batch: Vec<VectorDiff<RoomListItem>>) {
        let changes = apply_batch(&mut self.order, batch, |item| item.room_id().to_owned());
        for room in changes.removed {
            if self.forget(&room) {
                debug!(%room, "room left the list");
                let _ = self
                    .to_core
                    .send(Msg::RoomRemoved(RoomId::new(room.as_str())));
            }
        }
        let mut new_rooms = Vec::new();
        for item in changes.upserted {
            let room = item.into_inner();
            if !self.listed.contains_key(room.room_id())
                && !self.bridges.contains_key(room.room_id())
            {
                new_rooms.push(room.room_id().to_owned());
            }
            self.refresh(&room).await;
        }
        if !new_rooms.is_empty() {
            let _ = self.to_bridges.send(new_rooms);
        }
    }

    /// Reads the room's facts and sends its summary when they changed.
    async fn refresh(&mut self, room: &Room) {
        let id = room.room_id().to_owned();
        let bridge = self.bridges.get(&id).cloned().flatten();
        let (facts, parents) = match room_facts(&self.client, &self.settings, room, bridge).await {
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

    /// Whether the room was listed.
    fn forget(&mut self, room: &ruma::RoomId) -> bool {
        let Some(listed) = self.listed.remove(room) else {
            return false;
        };
        for parent in &listed.parents {
            self.spaces.remove_child(parent, room);
        }
        true
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

    fn apply_bridges(&mut self, states: BridgeStates) {
        for (room, bridge) in states {
            if let Some(listed) = self.listed.get_mut(&room)
                && listed.facts.bridge != bridge
            {
                debug!(%room, ?bridge, "bridge state");
                listed.facts.bridge = bridge.clone();
                let _ = self.to_core.send(Msg::RoomUpserted(summary(&listed.facts)));
            }
            self.bridges.insert(room, bridge);
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

/// What a batch of room list diffs changed.
#[derive(Debug, PartialEq, Eq)]
struct BatchChanges<T> {
    /// Rooms that were listed before the batch and are not after it.
    removed: Vec<OwnedRoomId>,
    /// Rooms the batch inserted, set or reset that are listed after it, once
    /// each with their latest value.
    upserted: Vec<T>,
}

/// Applies `batch` to `order`. A room that moves (a `Remove` and an `Insert`
/// in one batch) is upserted, not removed.
fn apply_batch<T: Clone>(
    order: &mut Vector<OwnedRoomId>,
    batch: Vec<VectorDiff<T>>,
    id: impl Fn(&T) -> OwnedRoomId,
) -> BatchChanges<T> {
    let mut gone = Vec::new();
    let mut touched = Vec::new();
    for diff in batch {
        match &diff {
            VectorDiff::Append { values } => touched.extend(values.iter().cloned()),
            VectorDiff::Reset { values } => {
                gone.extend(order.iter().cloned());
                touched.extend(values.iter().cloned());
            }
            VectorDiff::Clear => gone.extend(order.iter().cloned()),
            VectorDiff::PushFront { value } | VectorDiff::PushBack { value } => {
                touched.push(value.clone());
            }
            VectorDiff::Insert { value, .. } => touched.push(value.clone()),
            VectorDiff::Set { index, value } => {
                gone.extend(order.get(*index).cloned());
                touched.push(value.clone());
            }
            VectorDiff::PopFront => gone.extend(order.front().cloned()),
            VectorDiff::PopBack => gone.extend(order.back().cloned()),
            VectorDiff::Remove { index } => gone.extend(order.get(*index).cloned()),
            VectorDiff::Truncate { length } => gone.extend(order.iter().skip(*length).cloned()),
        }
        diff.map(|value| id(&value)).apply(order);
    }
    let listed: HashSet<&OwnedRoomId> = order.iter().collect();
    let mut seen = HashSet::new();
    let removed = gone
        .into_iter()
        .filter(|room| !listed.contains(room) && seen.insert(room.clone()))
        .collect();
    seen.clear();
    let mut upserted: Vec<T> = touched
        .into_iter()
        .rev()
        .filter(|value| {
            let room = id(value);
            listed.contains(&room) && seen.insert(room)
        })
        .collect();
    upserted.reverse();
    BatchChanges { removed, upserted }
}

async fn room_facts(
    client: &Client,
    settings: &NotificationSettings,
    room: &Room,
    bridge: Option<BridgeInfo>,
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
        bridge,
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

    /// Rooms in the mirror as `(room ID, version)`, so a `Set` is visible.
    fn apply(
        order: &mut Vector<OwnedRoomId>,
        batch: Vec<VectorDiff<(OwnedRoomId, u32)>>,
    ) -> BatchChanges<(OwnedRoomId, u32)> {
        apply_batch(order, batch, |(room, _)| room.clone())
    }

    #[test]
    fn a_moved_room_is_upserted_and_a_dropped_one_removed() {
        let (a, b, c) = (
            owned_room_id!("!a:example.org"),
            owned_room_id!("!b:example.org"),
            owned_room_id!("!c:example.org"),
        );
        let mut order = Vector::new();
        let changes = apply(
            &mut order,
            vec![VectorDiff::Reset {
                values: Vector::from(vec![(a.clone(), 1), (b.clone(), 1), (c.clone(), 1)]),
            }],
        );
        assert!(changes.removed.is_empty());
        assert_eq!(changes.upserted.len(), 3);

        // `c` moves to the front with new data, then is set again; `b` goes.
        let changes = apply(
            &mut order,
            vec![
                VectorDiff::Remove { index: 2 },
                VectorDiff::Insert {
                    index: 0,
                    value: (c.clone(), 2),
                },
                VectorDiff::Set {
                    index: 0,
                    value: (c.clone(), 3),
                },
                VectorDiff::Remove { index: 2 },
            ],
        );
        assert_eq!(changes.removed, vec![b]);
        assert_eq!(changes.upserted, vec![(c.clone(), 3)]);
        assert_eq!(order, Vector::from(vec![c.clone(), a.clone()]));

        let changes = apply(
            &mut order,
            vec![VectorDiff::Reset {
                values: Vector::from(vec![(a.clone(), 2)]),
            }],
        );
        assert_eq!(changes.removed, vec![c]);
        assert_eq!(changes.upserted, vec![(a, 2)]);
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

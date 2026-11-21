//! The room list: joined rooms in display order, room filters and the
//! resolution of `room.open` targets.

use std::cmp::Ordering;

use crate::fuzzy;
use crate::ids::RoomId;
use crate::room::{RoomName, RoomSummary, UnreadCounts};

/// One key of the room list's order (design, "Room list"). Each decides
/// only between the rooms that the keys before it leave tied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// Rooms with unread mentions first: whether a room has any, not how
    /// many, so a new mention does not reorder the rooms that have some.
    Mentions,
    /// Rooms with unread messages first, by the rule of `filter = "unread"`.
    Unread,
    /// Latest activity, newest first; rooms without activity last.
    Recent,
    /// The user's own latest read receipt, newest first; rooms never read
    /// last.
    Read,
    /// Case-insensitive by the full displayed name (`network / room`), which
    /// groups rooms by network.
    Name,
}

impl SortKey {
    pub const ALL: [SortKey; 5] = [
        SortKey::Mentions,
        SortKey::Unread,
        SortKey::Recent,
        SortKey::Read,
        SortKey::Name,
    ];

    /// The key in `ui.room_list.sort`: "mentions", "unread", "recent",
    /// "read", "name".
    pub fn config_name(self) -> &'static str {
        match self {
            SortKey::Mentions => "mentions",
            SortKey::Unread => "unread",
            SortKey::Recent => "recent",
            SortKey::Read => "read",
            SortKey::Name => "name",
        }
    }

    /// `Less` when `a` comes first. `None` sorts before `Some` and `false`
    /// before `true`, so comparing `b` with `a` puts the newest time and the
    /// rooms with something unread first, and rooms without a time last.
    fn compare(self, a: &RoomSummary, b: &RoomSummary) -> Ordering {
        let unread = |room: &RoomSummary| RoomFilter::Unread.matches(&room.unread);
        match self {
            SortKey::Mentions => (b.unread.mentions > 0).cmp(&(a.unread.mentions > 0)),
            SortKey::Unread => unread(b).cmp(&unread(a)),
            SortKey::Recent => b.latest_activity.cmp(&a.latest_activity),
            SortKey::Read => b.last_read.cmp(&a.last_read),
            SortKey::Name => lowercase(&a.name).cmp(lowercase(&b.name)),
        }
    }
}

/// The order of the room list: a non-empty list of distinct sort keys,
/// applied in order. Rooms that every key leaves tied are in `Name` order,
/// and rooms with the same name by room ID, so the order is total.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomSort {
    keys: Vec<SortKey>,
}

/// Why a list of keys is no `RoomSort`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomSortError {
    Empty,
    /// The first key that is listed a second time.
    Repeated(SortKey),
}

impl RoomSort {
    pub fn new(keys: Vec<SortKey>) -> Result<RoomSort, RoomSortError> {
        if keys.is_empty() {
            return Err(RoomSortError::Empty);
        }
        if let Some(index) = (1..keys.len()).find(|&index| keys[..index].contains(&keys[index])) {
            return Err(RoomSortError::Repeated(keys[index]));
        }
        Ok(RoomSort { keys })
    }

    pub fn one(key: SortKey) -> RoomSort {
        RoomSort { keys: vec![key] }
    }

    pub fn keys(&self) -> &[SortKey] {
        &self.keys
    }

    /// A total order: room IDs are unique and break every tie.
    fn compare(&self, a: &RoomSummary, b: &RoomSummary) -> Ordering {
        self.keys
            .iter()
            .map(|key| key.compare(a, b))
            .find(|order| order.is_ne())
            .unwrap_or_else(|| SortKey::Name.compare(a, b))
            .then_with(|| a.id.cmp(&b.id))
    }
}

impl Default for RoomSort {
    /// `"recent"`.
    fn default() -> RoomSort {
        RoomSort::one(SortKey::Recent)
    }
}

fn lowercase(name: &RoomName) -> impl Iterator<Item = char> {
    name.display_chars().flat_map(char::to_lowercase)
}

/// Which rooms a target or picker considers, by their unread counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RoomFilter {
    /// Every joined room.
    #[default]
    All,
    /// Any badge: messages, notifications or mentions above zero, or marked.
    Unread,
    /// Notifications or mentions above zero, or marked.
    Notify,
    /// Mentions above zero.
    Mentions,
}

impl RoomFilter {
    pub const ALL: [RoomFilter; 4] = [
        RoomFilter::All,
        RoomFilter::Unread,
        RoomFilter::Notify,
        RoomFilter::Mentions,
    ];

    /// The value of the `filter` argument: "all", "unread", "notify",
    /// "mentions".
    pub fn config_name(self) -> &'static str {
        match self {
            RoomFilter::All => "all",
            RoomFilter::Unread => "unread",
            RoomFilter::Notify => "notify",
            RoomFilter::Mentions => "mentions",
        }
    }

    pub fn matches(self, counts: &UnreadCounts) -> bool {
        match self {
            RoomFilter::All => true,
            RoomFilter::Unread => {
                counts.messages > 0
                    || counts.notifications > 0
                    || counts.mentions > 0
                    || counts.marked
            }
            RoomFilter::Notify => counts.notifications > 0 || counts.mentions > 0 || counts.marked,
            RoomFilter::Mentions => counts.mentions > 0,
        }
    }
}

/// Which of the rooms that pass a filter a `Pick` target chooses, in room
/// list order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pick {
    /// The first room.
    #[default]
    First,
    /// The first room after the current one, wrapping around.
    Next,
    /// The first room before the current one, wrapping around.
    Prev,
}

impl Pick {
    pub const ALL: [Pick; 3] = [Pick::First, Pick::Next, Pick::Prev];

    /// The value of the `pick` argument: "first", "next", "prev".
    pub fn config_name(self) -> &'static str {
        match self {
            Pick::First => "first",
            Pick::Next => "next",
            Pick::Prev => "prev",
        }
    }
}

/// Which room `room.open` and `picker.rooms` act on. A pick rule and a query
/// cannot be combined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomTarget {
    Pick { filter: RoomFilter, pick: Pick },
    Fuzzy { filter: RoomFilter, query: String },
}

/// Joined rooms in display order.
#[derive(Clone, Debug, Default)]
pub struct RoomList {
    sort: RoomSort,
    /// Ordered by `sort`, with unique ids.
    rooms: Vec<RoomSummary>,
}

impl RoomList {
    pub fn new(sort: RoomSort) -> Self {
        Self {
            sort,
            rooms: Vec::new(),
        }
    }

    /// Inserts or replaces the room with the same id and restores the order.
    /// Returns the room's old index (`None` for a new room) and its new index.
    pub fn upsert(&mut self, summary: RoomSummary) -> (Option<usize>, usize) {
        let old = self.remove(&summary.id).map(|(index, _)| index);
        (old, self.insert(summary))
    }

    /// Places a room that is not in the list; returns its index.
    fn insert(&mut self, summary: RoomSummary) -> usize {
        let index = self
            .rooms
            .partition_point(|room| self.sort.compare(room, &summary) == Ordering::Less);
        self.rooms.insert(index, summary);
        index
    }

    /// Returns the removed room and the index it had.
    pub fn remove(&mut self, id: &RoomId) -> Option<(usize, RoomSummary)> {
        let index = self.position(id)?;
        Some((index, self.rooms.remove(index)))
    }

    pub fn rooms(&self) -> &[RoomSummary] {
        &self.rooms
    }

    pub fn get(&self, id: &RoomId) -> Option<&RoomSummary> {
        self.rooms.iter().find(|room| room.id == *id)
    }

    pub fn position(&self, id: &RoomId) -> Option<usize> {
        self.rooms.iter().position(|room| room.id == *id)
    }

    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Optimistic clearing: sets the room's counts to zero (and `marked` to
    /// false) and places the room again, since the `mentions` and `unread`
    /// keys depend on them. Returns the room's old and new index.
    pub fn clear_unread(&mut self, id: &RoomId) -> Option<(usize, usize)> {
        let (old, mut room) = self.remove(id)?;
        room.unread = UnreadCounts::default();
        Some((old, self.insert(room)))
    }
}

/// The room `target` refers to, given the open room `current`.
///
/// `Next` and `Prev` never return `current` itself, so they return `None`
/// when no other room passes the filter. When `current` is `None` or not in
/// the list, `Next` behaves like `First` and `Prev` returns the last room
/// that passes the filter.
pub fn resolve_room(
    rooms: &RoomList,
    current: Option<&RoomId>,
    target: &RoomTarget,
) -> Option<RoomId> {
    let all = rooms.rooms();
    let found = match target {
        RoomTarget::Pick { filter, pick } => {
            let passes = |room: &&RoomSummary| filter.matches(&room.unread);
            let current = || current.and_then(|id| rooms.position(id));
            match pick {
                Pick::First => all.iter().find(passes),
                Pick::Next => match current() {
                    Some(i) => all[i + 1..].iter().chain(&all[..i]).find(passes),
                    None => all.iter().find(passes),
                },
                Pick::Prev => match current() {
                    Some(i) => all[..i]
                        .iter()
                        .rev()
                        .chain(all[i + 1..].iter().rev())
                        .find(passes),
                    None => all.iter().rev().find(passes),
                },
            }
        }
        RoomTarget::Fuzzy { filter, query } => {
            let candidates: Vec<&RoomSummary> = all
                .iter()
                .filter(|room| filter.matches(&room.unread))
                .collect();
            let names: Vec<String> = candidates
                .iter()
                .map(|room| room.name.to_string())
                .collect();
            fuzzy::rank(query, &names)
                .first()
                .map(|&best| candidates[best])
        }
    };
    found.map(|room| room.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{RoomId, Timestamp};
    use crate::room::{NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts};

    fn room(
        id: &str,
        network: Option<&str>,
        name: &str,
        at: Option<u64>,
        unread: UnreadCounts,
    ) -> RoomSummary {
        RoomSummary {
            id: RoomId::new(id),
            name: RoomName {
                network: network.map(Into::into),
                room: name.into(),
            },
            kind: RoomKind::Group,
            unread,
            mode: NotificationMode::Default,
            latest_activity: at.map(Timestamp::from_millis),
            last_read: None,
        }
    }

    fn unread(messages: u64) -> UnreadCounts {
        UnreadCounts {
            messages,
            ..UnreadCounts::default()
        }
    }

    fn mentions(mentions: u64) -> UnreadCounts {
        UnreadCounts {
            messages: mentions,
            notifications: mentions,
            mentions,
            marked: false,
        }
    }

    fn read_at(summary: RoomSummary, at: Option<u64>) -> RoomSummary {
        RoomSummary {
            last_read: at.map(Timestamp::from_millis),
            ..summary
        }
    }

    fn ids(list: &RoomList) -> Vec<&str> {
        list.rooms().iter().map(|r| r.id.as_str()).collect()
    }

    fn list(sort: RoomSort, rooms: Vec<RoomSummary>) -> RoomList {
        let mut list = RoomList::new(sort);
        for r in rooms {
            list.upsert(r);
        }
        list
    }

    fn recent() -> RoomSort {
        RoomSort::one(SortKey::Recent)
    }

    /// The room IDs in the order of `keys`.
    fn order(keys: &[SortKey], rooms: Vec<RoomSummary>) -> Vec<String> {
        let list = list(RoomSort::new(keys.to_vec()).unwrap(), rooms);
        list.rooms()
            .iter()
            .map(|r| r.id.as_str().to_owned())
            .collect()
    }

    #[test]
    fn recent_order_is_newest_first_with_inactive_rooms_last() {
        let l = list(
            recent(),
            vec![
                room("!a:example.org", None, "a", Some(10), unread(0)),
                room("!b:example.org", None, "b", None, unread(0)),
                room("!c:example.org", None, "c", Some(30), unread(0)),
            ],
        );
        assert_eq!(
            ids(&l),
            ["!c:example.org", "!a:example.org", "!b:example.org"]
        );
    }

    #[test]
    fn recent_ties_fall_back_to_the_name_order() {
        let l = list(
            recent(),
            vec![
                room("!z:example.org", None, "Same", Some(10), unread(0)),
                room("!y:example.org", None, "same", Some(10), unread(0)),
                room("!x:example.org", None, "Other", Some(10), unread(0)),
                room("!w:example.org", None, "Idle", None, unread(0)),
                room("!v:example.org", None, "Away", None, unread(0)),
            ],
        );
        assert_eq!(
            ids(&l),
            [
                "!x:example.org",
                "!y:example.org",
                "!z:example.org",
                "!v:example.org",
                "!w:example.org",
            ]
        );
    }

    #[test]
    fn name_order_is_case_insensitive_on_the_full_name() {
        let l = list(
            RoomSort::one(SortKey::Name),
            vec![
                room(
                    "!1:example.org",
                    Some("Work"),
                    "#general",
                    Some(1),
                    unread(0),
                ),
                room(
                    "!2:example.org",
                    Some("home"),
                    "#random",
                    Some(2),
                    unread(0),
                ),
                room("!3:example.org", None, "Alice", Some(3), unread(0)),
            ],
        );
        assert_eq!(
            ids(&l),
            ["!3:example.org", "!2:example.org", "!1:example.org"]
        );
    }

    #[test]
    fn mentions_puts_rooms_with_any_mention_first_by_presence_not_count() {
        // Rooms with 1 and 5 mentions are tied, and so are the others,
        // whatever else is unread: each group goes by name, not by activity.
        let rooms = vec![
            room("!d:example.org", None, "delta", Some(40), unread(3)),
            room("!b:example.org", None, "bravo", Some(20), mentions(5)),
            room("!c:example.org", None, "charlie", None, unread(0)),
            room("!a:example.org", None, "alpha", Some(10), mentions(1)),
        ];
        assert_eq!(
            order(&[SortKey::Mentions], rooms),
            [
                "!a:example.org",
                "!b:example.org",
                "!c:example.org",
                "!d:example.org"
            ]
        );
    }

    #[test]
    fn unread_puts_rooms_with_any_badge_first_marked_or_counted_alike() {
        let marked = UnreadCounts {
            marked: true,
            ..UnreadCounts::default()
        };
        let rooms = vec![
            room("!a:example.org", None, "alpha", Some(40), unread(0)),
            room("!b:example.org", None, "bravo", Some(10), marked),
            room("!c:example.org", None, "charlie", Some(20), unread(1)),
            room("!d:example.org", None, "delta", Some(30), unread(7)),
        ];
        assert_eq!(
            order(&[SortKey::Unread], rooms),
            [
                "!b:example.org",
                "!c:example.org",
                "!d:example.org",
                "!a:example.org"
            ]
        );
    }

    #[test]
    fn recent_and_read_put_the_newest_time_first_and_rooms_without_one_last() {
        let rooms = || {
            vec![
                read_at(room("!a:example.org", None, "a", Some(30), unread(0)), None),
                read_at(room("!b:example.org", None, "b", None, unread(0)), Some(20)),
                read_at(
                    room("!c:example.org", None, "c", Some(10), unread(0)),
                    Some(10),
                ),
                read_at(
                    room("!d:example.org", None, "d", Some(20), unread(0)),
                    Some(30),
                ),
            ]
        };
        assert_eq!(
            order(&[SortKey::Recent], rooms()),
            [
                "!a:example.org",
                "!d:example.org",
                "!c:example.org",
                "!b:example.org"
            ]
        );
        assert_eq!(
            order(&[SortKey::Read], rooms()),
            [
                "!d:example.org",
                "!b:example.org",
                "!c:example.org",
                "!a:example.org"
            ]
        );
    }

    #[test]
    fn keys_apply_in_order_each_between_the_rooms_the_ones_before_leave_tied() {
        let rooms = || {
            vec![
                // Read, recently.
                read_at(
                    room("!a:example.org", None, "a", Some(50), unread(0)),
                    Some(40),
                ),
                // Unread, read long ago.
                read_at(
                    room("!b:example.org", None, "b", Some(10), unread(2)),
                    Some(10),
                ),
                // Unread, read recently.
                read_at(
                    room("!c:example.org", None, "c", Some(20), unread(1)),
                    Some(30),
                ),
                // A mention, read long ago.
                read_at(
                    room("!d:example.org", None, "d", Some(30), mentions(1)),
                    Some(5),
                ),
                // A mention, read more recently.
                read_at(
                    room("!e:example.org", None, "e", Some(40), mentions(2)),
                    Some(20),
                ),
                // Never read.
                read_at(room("!f:example.org", None, "f", Some(60), unread(0)), None),
            ]
        };
        assert_eq!(
            order(
                &[SortKey::Mentions, SortKey::Unread, SortKey::Read],
                rooms()
            ),
            [
                "!e:example.org",
                "!d:example.org",
                "!c:example.org",
                "!b:example.org",
                "!a:example.org",
                "!f:example.org"
            ]
        );
        // Rooms with mentions are unread too; the name decides the rest.
        assert_eq!(
            order(&[SortKey::Unread, SortKey::Mentions], rooms()),
            [
                "!d:example.org",
                "!e:example.org",
                "!b:example.org",
                "!c:example.org",
                "!a:example.org",
                "!f:example.org"
            ]
        );
    }

    #[test]
    fn rooms_every_key_leaves_tied_go_by_name_then_by_room_id() {
        let rooms = vec![
            room("!z:example.org", None, "Same", Some(40), unread(1)),
            room("!y:example.org", None, "same", Some(10), unread(2)),
            room("!x:example.org", None, "Other", Some(30), unread(0)),
            room("!w:example.org", None, "Another", Some(20), unread(3)),
        ];
        assert_eq!(
            order(&[SortKey::Unread], rooms),
            [
                "!w:example.org",
                "!y:example.org",
                "!z:example.org",
                "!x:example.org"
            ]
        );
    }

    #[test]
    fn a_sort_needs_at_least_one_key_and_each_key_once() {
        use SortKey::{Mentions, Read, Unread};
        assert_eq!(RoomSort::new(vec![]), Err(RoomSortError::Empty));
        assert_eq!(
            RoomSort::new(vec![Unread, Read, Unread]),
            Err(RoomSortError::Repeated(Unread))
        );
        assert_eq!(
            RoomSort::new(vec![Mentions, Read]).unwrap().keys(),
            [Mentions, Read]
        );
        assert_eq!(RoomSort::default().keys(), [SortKey::Recent]);
    }

    #[test]
    fn every_sort_key_has_a_name_of_its_own() {
        for key in SortKey::ALL {
            let named: Vec<SortKey> = SortKey::ALL
                .into_iter()
                .filter(|other| other.config_name() == key.config_name())
                .collect();
            assert_eq!(named, [key]);
        }
    }

    #[test]
    fn clear_unread_places_the_room_again() {
        let mut l = list(
            RoomSort::one(SortKey::Unread),
            vec![
                room("!a:example.org", None, "a", Some(10), unread(1)),
                room("!b:example.org", None, "b", Some(20), unread(2)),
                room("!c:example.org", None, "c", Some(30), unread(0)),
            ],
        );
        let a = RoomId::new("!a:example.org");
        assert_eq!(l.clear_unread(&a), Some((0, 1)));
        assert_eq!(
            ids(&l),
            ["!b:example.org", "!a:example.org", "!c:example.org"]
        );
        assert_eq!(l.get(&a).unwrap().unread, UnreadCounts::default());
        assert_eq!(l.clear_unread(&RoomId::new("!gone:example.org")), None);
    }

    #[test]
    fn upsert_replaces_and_reorders() {
        let mut l = list(
            recent(),
            vec![
                room("!a:example.org", None, "a", Some(10), unread(0)),
                room("!b:example.org", None, "b", Some(20), unread(0)),
            ],
        );
        // !a moves from below !b to the top.
        let moved = l.upsert(room("!a:example.org", None, "a", Some(30), unread(1)));
        assert_eq!(moved, (Some(1), 0));
        assert_eq!(ids(&l), ["!a:example.org", "!b:example.org"]);
        assert_eq!(l.len(), 2);
        let added = l.upsert(room("!c:example.org", None, "c", Some(25), unread(0)));
        assert_eq!(added, (None, 1));
        assert_eq!(
            ids(&l),
            ["!a:example.org", "!c:example.org", "!b:example.org"]
        );
    }

    #[test]
    fn remove_returns_the_index_the_room_had() {
        let mut l = list(
            recent(),
            vec![
                room("!a:example.org", None, "a", Some(10), unread(0)),
                room("!b:example.org", None, "b", Some(20), unread(0)),
            ],
        );
        let (index, removed) = l.remove(&RoomId::new("!a:example.org")).unwrap();
        assert_eq!((index, removed.id.as_str()), (1, "!a:example.org"));
        assert_eq!(ids(&l), ["!b:example.org"]);
        assert_eq!(l.remove(&RoomId::new("!a:example.org")), None);
    }

    #[test]
    fn filters() {
        let marked = UnreadCounts {
            marked: true,
            ..UnreadCounts::default()
        };
        let notif = UnreadCounts {
            messages: 1,
            notifications: 1,
            ..UnreadCounts::default()
        };
        let mention = UnreadCounts {
            messages: 1,
            notifications: 1,
            mentions: 1,
            marked: false,
        };
        assert!(RoomFilter::All.matches(&UnreadCounts::default()));
        assert!(RoomFilter::Unread.matches(&unread(1)));
        assert!(RoomFilter::Unread.matches(&marked));
        assert!(!RoomFilter::Unread.matches(&UnreadCounts::default()));
        assert!(!RoomFilter::Notify.matches(&unread(1)));
        assert!(RoomFilter::Notify.matches(&notif));
        assert!(RoomFilter::Notify.matches(&marked));
        assert!(!RoomFilter::Mentions.matches(&notif));
        assert!(RoomFilter::Mentions.matches(&mention));
    }

    fn three_unread() -> RoomList {
        // Order: c, b, a (newest first). b is read.
        list(
            recent(),
            vec![
                room("!a:example.org", None, "a", Some(10), unread(1)),
                room("!b:example.org", None, "b", Some(20), unread(0)),
                room("!c:example.org", None, "c", Some(30), unread(2)),
            ],
        )
    }

    fn pick(filter: RoomFilter, pick: Pick) -> RoomTarget {
        RoomTarget::Pick { filter, pick }
    }

    #[test]
    fn pick_without_a_current_room_in_the_list() {
        let l = three_unread();
        let id = |s: &str| Some(RoomId::new(s));
        assert_eq!(
            resolve_room(&l, None, &pick(RoomFilter::Unread, Pick::First)),
            id("!c:example.org")
        );
        // A stale open room, for example after the room was removed.
        let gone = RoomId::new("!gone:example.org");
        for current in [None, Some(&gone)] {
            assert_eq!(
                resolve_room(&l, current, &pick(RoomFilter::Unread, Pick::Next)),
                id("!c:example.org")
            );
            assert_eq!(
                resolve_room(&l, current, &pick(RoomFilter::Unread, Pick::Prev)),
                id("!a:example.org")
            );
        }
    }

    #[test]
    fn pick_next_wraps_around_and_skips_current() {
        let l = three_unread();
        let a = RoomId::new("!a:example.org");
        let c = RoomId::new("!c:example.org");
        assert_eq!(
            resolve_room(&l, Some(&c), &pick(RoomFilter::Unread, Pick::Next)),
            Some(a.clone())
        );
        assert_eq!(
            resolve_room(&l, Some(&a), &pick(RoomFilter::Unread, Pick::Next)),
            Some(c.clone())
        );
        assert_eq!(
            resolve_room(&l, Some(&c), &pick(RoomFilter::Unread, Pick::Prev)),
            Some(a.clone())
        );
    }

    #[test]
    fn pick_next_from_a_read_current_room() {
        let l = three_unread();
        let b = RoomId::new("!b:example.org");
        assert_eq!(
            resolve_room(&l, Some(&b), &pick(RoomFilter::Unread, Pick::Next)),
            Some(RoomId::new("!a:example.org"))
        );
    }

    #[test]
    fn pick_next_returns_none_when_only_current_matches() {
        let l = list(
            recent(),
            vec![
                room("!a:example.org", None, "a", Some(10), unread(1)),
                room("!b:example.org", None, "b", Some(20), unread(0)),
            ],
        );
        let a = RoomId::new("!a:example.org");
        assert_eq!(
            resolve_room(&l, Some(&a), &pick(RoomFilter::Unread, Pick::Next)),
            None
        );
    }

    #[test]
    fn clear_unread_makes_the_next_pick_skip_the_room() {
        let mut l = three_unread();
        let c = RoomId::new("!c:example.org");
        l.clear_unread(&c);
        assert_eq!(l.get(&c).unwrap().unread, UnreadCounts::default());
        assert_eq!(
            resolve_room(&l, None, &pick(RoomFilter::Unread, Pick::First)),
            Some(RoomId::new("!a:example.org"))
        );
    }

    #[test]
    fn fuzzy_target_matches_the_displayed_name_within_the_filter() {
        let l = list(
            recent(),
            vec![
                room(
                    "!1:example.org",
                    Some("work"),
                    "#general",
                    Some(10),
                    unread(0),
                ),
                room(
                    "!2:example.org",
                    Some("home"),
                    "#general",
                    Some(20),
                    unread(3),
                ),
            ],
        );
        let all = RoomTarget::Fuzzy {
            filter: RoomFilter::All,
            query: "work gen".into(),
        };
        assert_eq!(
            resolve_room(&l, None, &all),
            Some(RoomId::new("!1:example.org"))
        );
        let unread_only = RoomTarget::Fuzzy {
            filter: RoomFilter::Unread,
            query: "general".into(),
        };
        assert_eq!(
            resolve_room(&l, None, &unread_only),
            Some(RoomId::new("!2:example.org"))
        );
        let none = RoomTarget::Fuzzy {
            filter: RoomFilter::All,
            query: "zzzz".into(),
        };
        assert_eq!(resolve_room(&l, None, &none), None);
    }

    #[test]
    fn every_displayed_name_resolves_to_its_own_room() {
        let l = list(
            recent(),
            vec![
                room(
                    "!alerts:example.org",
                    Some("Telegram"),
                    "!alerts",
                    Some(10),
                    unread(0),
                ),
                room(
                    "!general:example.org",
                    Some("Telegram"),
                    "general",
                    Some(20),
                    unread(0),
                ),
                room(
                    "!club:example.org",
                    Some("Telegram"),
                    "^_^ club",
                    Some(30),
                    unread(0),
                ),
                room("!dev:example.org", Some("Work"), "dev", Some(40), unread(0)),
                // Newer, so it comes before `Work / dev` in the list.
                room(
                    "!devops:example.org",
                    Some("Work"),
                    "devops",
                    Some(50),
                    unread(0),
                ),
            ],
        );
        for r in l.rooms() {
            let target = RoomTarget::Fuzzy {
                filter: RoomFilter::All,
                query: r.name.to_string(),
            };
            assert_eq!(
                resolve_room(&l, None, &target).as_ref(),
                Some(&r.id),
                "{}",
                r.name
            );
        }
    }
}

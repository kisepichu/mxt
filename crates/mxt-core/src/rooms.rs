//! The room list: joined rooms in display order, room filters and the
//! resolution of `room.open` targets.

use std::cmp::Ordering;

use crate::fuzzy;
use crate::ids::{RoomId, Timestamp};
use crate::room::{RoomName, RoomSummary, UnreadCounts};

/// One key of the room list's order (design, "Room list"). Each decides
/// only between the rooms that the keys before it leave tied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// Rooms with unread mentions first: whether a room has any, not how
    /// many, so a new mention does not reorder the rooms that have some.
    Mentions,
    /// Rooms that notify first, by the rule of `filter = "notify"`:
    /// notifications or mentions, or the manual unread mark, so a room whose
    /// only unread state is a dim count is not among them.
    Notify,
    /// Rooms with unread messages first, by the rule of `filter = "unread"`.
    Unread,
    /// Latest activity, newest first; rooms without activity last.
    Recent,
    /// The latest news for you, newest first: an unread room by the time of
    /// its newest message, any other room by the user's own latest read
    /// receipt (see `news_time`); rooms without such a time last.
    Read,
    /// Case-insensitive by the full displayed name (`network / room`), which
    /// groups rooms by network.
    Name,
}

impl SortKey {
    pub const ALL: [SortKey; 6] = [
        SortKey::Mentions,
        SortKey::Notify,
        SortKey::Unread,
        SortKey::Recent,
        SortKey::Read,
        SortKey::Name,
    ];

    /// The key in `ui.room_list.sort`: "mentions", "notify", "unread",
    /// "recent", "read", "name".
    pub fn config_name(self) -> &'static str {
        match self {
            SortKey::Mentions => "mentions",
            SortKey::Notify => "notify",
            SortKey::Unread => "unread",
            SortKey::Recent => "recent",
            SortKey::Read => "read",
            SortKey::Name => "name",
        }
    }

    /// `Less` when `a` comes first. `None` sorts before `Some` and `false`
    /// before `true`, so comparing `b` with `a` puts the newest time and the
    /// rooms with something unread first, and rooms without a time last.
    /// The room `open` has nothing unread for `Mentions`, `Notify` and
    /// `Unread`, and counts as read up to its latest activity for `Read`
    /// (design, "The open room sorts as read").
    fn compare(self, a: &RoomSummary, b: &RoomSummary, open: Option<&RoomId>) -> Ordering {
        let has = |room: &RoomSummary, filter: RoomFilter| {
            open != Some(&room.id) && filter.matches(&room.unread)
        };
        match self {
            SortKey::Mentions => has(b, RoomFilter::Mentions).cmp(&has(a, RoomFilter::Mentions)),
            SortKey::Notify => has(b, RoomFilter::Notify).cmp(&has(a, RoomFilter::Notify)),
            SortKey::Unread => has(b, RoomFilter::Unread).cmp(&has(a, RoomFilter::Unread)),
            SortKey::Recent => b.latest_activity.cmp(&a.latest_activity),
            SortKey::Read => news_time(b, open).cmp(&news_time(a, open)),
            SortKey::Name => lowercase(&a.name).cmp(lowercase(&b.name)),
        }
    }
}

/// A room's time under `SortKey::Read` (design, "Room list"): for a room
/// unread by the rule of `RoomFilter::Unread` (any badge), its latest
/// activity, the time of its newest message, or its `last_read` when it has
/// none; for any other room its `last_read`. The open room sorts as read: the
/// later of its `last_read` and its latest activity; its receipt, when it has
/// something to read, lifts it to the top of the read rooms (see
/// `RoomList::clear_unread`).
fn news_time(room: &RoomSummary, open: Option<&RoomId>) -> Option<Timestamp> {
    if open == Some(&room.id) {
        room.last_read.max(room.latest_activity)
    } else if RoomFilter::Unread.matches(&room.unread) {
        room.latest_activity.or(room.last_read)
    } else {
        room.last_read
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

    /// A total order: room IDs are unique and break every tie.
    fn compare(&self, a: &RoomSummary, b: &RoomSummary, open: Option<&RoomId>) -> Ordering {
        self.keys
            .iter()
            .map(|key| key.compare(a, b, open))
            .find(|order| order.is_ne())
            .unwrap_or_else(|| SortKey::Name.compare(a, b, open))
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
    /// Notifications or mentions above zero, or marked: the rooms that
    /// notify. The `notify` sort key uses this rule too.
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
/// list order. None of them chooses the current (open) room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pick {
    /// The first room other than the current one: the topmost one, which
    /// `]u` opens (design, "Action registry").
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
    /// The open room, which sorts as read (see `SortKey::compare`).
    open: Option<RoomId>,
    /// Ordered by `sort`, with unique ids.
    rooms: Vec<RoomSummary>,
}

impl RoomList {
    pub fn new(sort: RoomSort) -> Self {
        Self {
            sort,
            open: None,
            rooms: Vec::new(),
        }
    }

    /// Inserts or replaces the room with the same id and restores the order.
    /// A replaced room keeps the later of its stored and its updated
    /// `last_read`: receipts only move forward, so facts read before a
    /// receipt's echo do not move the room back (design, "Optimistic
    /// clearing"). Returns the room's old index (`None` for a new room) and
    /// its new index.
    pub fn upsert(&mut self, mut summary: RoomSummary) -> (Option<usize>, usize) {
        let old = self.take(&summary.id).map(|(index, stored)| {
            summary.last_read = summary.last_read.max(stored.last_read);
            index
        });
        (old, self.insert(summary))
    }

    /// Places a room that is not in the list; returns its index.
    fn insert(&mut self, summary: RoomSummary) -> usize {
        let open = self.open.as_ref();
        let index = self
            .rooms
            .partition_point(|room| self.sort.compare(room, &summary, open) == Ordering::Less);
        self.rooms.insert(index, summary);
        index
    }

    /// Takes a room out to place it again; returns it and the index it had.
    fn take(&mut self, id: &RoomId) -> Option<(usize, RoomSummary)> {
        let index = self.position(id)?;
        Some((index, self.rooms.remove(index)))
    }

    /// Returns the removed room and the index it had. A removed open room is
    /// no longer open.
    pub fn remove(&mut self, id: &RoomId) -> Option<(usize, RoomSummary)> {
        self.open.take_if(|open| open == id);
        self.take(id)
    }

    /// Makes `id` the open room and places it and the previously open room
    /// again. The open room sorts as read (design, "The open room sorts as
    /// read"): nothing unread for the `mentions`, `notify` and `unread` keys,
    /// and read up to its latest activity for `read`. Returns `id`'s new
    /// index.
    pub fn open(&mut self, id: &RoomId) -> Option<usize> {
        // Both rooms out first: the others keep their order with either
        // room open.
        let room = self.take(id);
        if let Some((_, previous)) = self
            .open
            .replace(id.clone())
            .and_then(|previous| self.take(&previous))
        {
            self.insert(previous);
        }
        room.map(|(_, room)| self.insert(room))
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

    /// Optimistic clearing (design, "Optimistic clearing"): sets the room's
    /// counts to zero (and `marked` to false) and places the room again,
    /// since the `mentions`, `notify`, `unread` and `read` keys depend on
    /// them. `receipt_at` is the time of the event the receipt goes on. A
    /// room with something to read (a badge, or that event after its
    /// `last_read`) is read now: the echo of its receipt brings the
    /// homeserver's time of taking it, which is no earlier than the room's
    /// latest activity or any other time the list holds (clocks ahead of the
    /// homeserver's aside). So its `last_read` goes up to the later of its
    /// latest activity and 1 ms after the newest time `SortKey::Read` gives
    /// another room, 1 ms keeping it off a tie with that room: the room tops
    /// the read rooms at once, and the echo keeps it there. A room with
    /// nothing to read keeps its `last_read` and its place: the homeserver
    /// ignores a receipt that does not move forward, so no echo comes, when
    /// the user's newest receipt is the same kind of unthreaded receipt mxt
    /// sends; a room last read with another kind (Element's main-thread
    /// receipts, the other receipt type) moves once its echo comes (design,
    /// "Risks"). Returns the room's old and new index.
    pub fn clear_unread(
        &mut self,
        id: &RoomId,
        receipt_at: Option<Timestamp>,
    ) -> Option<(usize, usize)> {
        let (old, mut room) = self.take(id)?;
        if RoomFilter::Unread.matches(&room.unread) || receipt_at > room.last_read {
            let open = self.open.as_ref();
            let newest = self
                .rooms
                .iter()
                .filter_map(|other| news_time(other, open))
                .max();
            let after =
                newest.map(|time| Timestamp::from_millis(time.as_millis().saturating_add(1)));
            room.last_read = room.last_read.max(room.latest_activity).max(after);
        }
        room.unread = UnreadCounts::default();
        Some((old, self.insert(room)))
    }
}

/// The room `target` refers to, given the open room `current`.
///
/// No `Pick` returns `current` itself, so each returns `None` when no other
/// room passes the filter. When `current` is `None` or not in the list,
/// `First` and `Next` return the first room that passes the filter and
/// `Prev` the last.
pub fn resolve_room(
    rooms: &RoomList,
    current: Option<&RoomId>,
    target: &RoomTarget,
) -> Option<RoomId> {
    let all = rooms.rooms();
    let found = match target {
        RoomTarget::Pick { filter, pick } => {
            let passes = |room: &&RoomSummary| filter.matches(&room.unread);
            let index = || current.and_then(|id| rooms.position(id));
            match pick {
                Pick::First => all
                    .iter()
                    .find(|room| Some(&room.id) != current && passes(room)),
                Pick::Next => match index() {
                    Some(i) => all[i + 1..].iter().chain(&all[..i]).find(passes),
                    None => all.iter().find(passes),
                },
                Pick::Prev => match index() {
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
    use crate::room::{Badge, NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts};

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

    /// By group under `"mentions, notify, unread, read"`: `!a` has a
    /// mention; `!b` (notifications) and `!c` (marked unread) notify; `!d`,
    /// muted, has a dim count only, and the newest message; `!e` is read.
    /// Each room's latest activity and last read keep the same order, so
    /// `read` orders the groups alike by either.
    fn notify_rooms() -> Vec<RoomSummary> {
        let notifications = UnreadCounts {
            messages: 2,
            notifications: 2,
            ..UnreadCounts::default()
        };
        let marked = UnreadCounts {
            marked: true,
            ..UnreadCounts::default()
        };
        vec![
            read_at(
                room("!a:example.org", None, "a", Some(10), mentions(1)),
                Some(10),
            ),
            read_at(
                room("!b:example.org", None, "b", Some(15), notifications),
                Some(12),
            ),
            read_at(
                room("!c:example.org", None, "c", Some(25), marked),
                Some(20),
            ),
            RoomSummary {
                mode: NotificationMode::Mute,
                ..read_at(
                    room("!d:example.org", None, "d", Some(60), unread(5)),
                    Some(50),
                )
            },
            read_at(
                room("!e:example.org", None, "e", Some(40), unread(0)),
                Some(40),
            ),
        ]
    }

    fn notify_first() -> RoomSort {
        RoomSort::new(vec![
            SortKey::Mentions,
            SortKey::Notify,
            SortKey::Unread,
            SortKey::Read,
        ])
        .unwrap()
    }

    #[test]
    fn notify_puts_the_rooms_that_notify_before_the_rooms_with_only_a_dim_count() {
        // Without `notify`, the muted room shares the unread group.
        assert_eq!(
            order(
                &[SortKey::Mentions, SortKey::Unread, SortKey::Read],
                notify_rooms()
            ),
            [
                "!a:example.org",
                "!d:example.org",
                "!c:example.org",
                "!b:example.org",
                "!e:example.org"
            ]
        );
        let mut l = list(notify_first(), notify_rooms());
        assert_eq!(
            ids(&l),
            [
                "!a:example.org",
                "!c:example.org",
                "!b:example.org",
                "!d:example.org",
                "!e:example.org"
            ]
        );
        // The dim count is still unread, with its dim badge.
        let muted = l.get(&RoomId::new("!d:example.org")).unwrap().clone();
        assert!(RoomFilter::Unread.matches(&muted.unread));
        assert_eq!(Badge::from_counts(&muted.unread), Some(Badge::Messages(5)));
        // Unmuted, its next message notifies: the room moves among the rooms
        // that notify.
        let notifying = RoomSummary {
            mode: NotificationMode::Default,
            unread: UnreadCounts {
                messages: 6,
                notifications: 1,
                ..UnreadCounts::default()
            },
            latest_activity: Some(Timestamp::from_millis(70)),
            ..muted
        };
        assert_eq!(l.upsert(notifying), (Some(3), 1));
        assert_eq!(
            ids(&l),
            [
                "!a:example.org",
                "!d:example.org",
                "!c:example.org",
                "!b:example.org",
                "!e:example.org"
            ]
        );
    }

    #[test]
    fn under_notify_the_open_room_sorts_as_read_until_another_room_opens() {
        let mut l = list(notify_first(), notify_rooms());
        let b = RoomId::new("!b:example.org");
        assert_eq!(l.open(&b), Some(4));
        assert_eq!(
            ids(&l),
            [
                "!a:example.org",
                "!c:example.org",
                "!d:example.org",
                "!e:example.org",
                "!b:example.org"
            ]
        );
        // The filters still see its notifications.
        assert!(RoomFilter::Notify.matches(&l.get(&b).unwrap().unread));
        l.open(&RoomId::new("!e:example.org"));
        assert_eq!(
            ids(&l),
            [
                "!a:example.org",
                "!c:example.org",
                "!b:example.org",
                "!d:example.org",
                "!e:example.org"
            ]
        );
    }

    #[test]
    fn read_orders_unread_rooms_by_their_newest_message_and_the_others_by_the_last_read() {
        let rooms = vec![
            read_at(
                room("!a:example.org", None, "a", Some(30), unread(0)),
                Some(30),
            ),
            // Unread since 40, read long before.
            read_at(
                room("!b:example.org", None, "b", Some(40), unread(2)),
                Some(10),
            ),
            read_at(
                room("!c:example.org", None, "c", Some(50), unread(0)),
                Some(50),
            ),
            // Unread without a latest activity: by its last read.
            read_at(room("!d:example.org", None, "d", None, unread(1)), Some(20)),
            // Never read and nothing unread: last, whatever its activity.
            read_at(room("!e:example.org", None, "e", Some(60), unread(0)), None),
        ];
        assert_eq!(
            order(&[SortKey::Read], rooms),
            [
                "!c:example.org",
                "!b:example.org",
                "!a:example.org",
                "!d:example.org",
                "!e:example.org"
            ]
        );
    }

    #[test]
    fn a_room_that_gets_a_new_message_goes_to_the_top_of_its_unread_group() {
        // The unread rooms by newest message are !c, !d, !b; by last read
        // they would be !c, !b, !d.
        let mut l = list(
            RoomSort::new(vec![SortKey::Mentions, SortKey::Unread, SortKey::Read]).unwrap(),
            vec![
                read_at(
                    room("!a:example.org", None, "a", Some(10), mentions(1)),
                    Some(5),
                ),
                read_at(
                    room("!b:example.org", None, "b", Some(20), unread(2)),
                    Some(3),
                ),
                read_at(
                    room("!c:example.org", None, "c", Some(40), unread(1)),
                    Some(30),
                ),
                read_at(
                    room("!d:example.org", None, "d", Some(30), unread(4)),
                    Some(1),
                ),
                read_at(
                    room("!e:example.org", None, "e", Some(25), unread(0)),
                    Some(25),
                ),
                read_at(
                    room("!f:example.org", None, "f", Some(15), unread(0)),
                    Some(15),
                ),
            ],
        );
        assert_eq!(
            ids(&l),
            [
                "!a:example.org",
                "!c:example.org",
                "!d:example.org",
                "!b:example.org",
                "!e:example.org",
                "!f:example.org"
            ]
        );
        // A new message in !f, which you read before the others.
        let news = read_at(
            room("!f:example.org", None, "f", Some(60), unread(1)),
            Some(15),
        );
        assert_eq!(l.upsert(news), (Some(5), 1));
        assert_eq!(
            ids(&l),
            [
                "!a:example.org",
                "!f:example.org",
                "!c:example.org",
                "!d:example.org",
                "!b:example.org",
                "!e:example.org"
            ]
        );
    }

    #[test]
    fn under_notify_a_room_that_gets_a_new_message_goes_to_the_top_of_its_group() {
        let mut rooms = notify_rooms();
        rooms.push(read_at(
            room("!f:example.org", None, "f", Some(5), unread(0)),
            Some(5),
        ));
        rooms.push(read_at(
            room("!g:example.org", None, "g", Some(8), unread(0)),
            Some(8),
        ));
        let mut l = list(notify_first(), rooms);
        assert_eq!(
            ids(&l),
            [
                "!a:example.org",
                "!c:example.org",
                "!b:example.org",
                "!d:example.org",
                "!e:example.org",
                "!g:example.org",
                "!f:example.org"
            ]
        );
        // A message that notifies: the top of the rooms that notify.
        let notifying = UnreadCounts {
            messages: 1,
            notifications: 1,
            ..UnreadCounts::default()
        };
        let news = read_at(
            room("!f:example.org", None, "f", Some(70), notifying),
            Some(5),
        );
        assert_eq!(l.upsert(news), (Some(6), 1));
        // A message with a dim count: the top of the rooms with only one.
        let news = read_at(
            room("!g:example.org", None, "g", Some(80), unread(1)),
            Some(8),
        );
        assert_eq!(l.upsert(news), (Some(6), 4));
        assert_eq!(
            ids(&l),
            [
                "!a:example.org",
                "!f:example.org",
                "!c:example.org",
                "!b:example.org",
                "!g:example.org",
                "!d:example.org",
                "!e:example.org"
            ]
        );
    }

    #[test]
    fn under_read_the_open_room_counts_as_read_up_to_its_latest_activity() {
        let marked = UnreadCounts {
            marked: true,
            ..UnreadCounts::default()
        };
        let mut l = list(
            RoomSort::one(SortKey::Read),
            vec![
                read_at(
                    room("!a:example.org", None, "a", Some(30), unread(0)),
                    Some(30),
                ),
                // Nothing unread, with activity after the last receipt.
                read_at(
                    room("!b:example.org", None, "b", Some(45), unread(0)),
                    Some(10),
                ),
                // Marked unread after it was read.
                read_at(
                    room("!c:example.org", None, "c", Some(20), marked),
                    Some(50),
                ),
                read_at(
                    room("!d:example.org", None, "d", Some(40), unread(0)),
                    Some(40),
                ),
            ],
        );
        assert_eq!(
            ids(&l),
            [
                "!d:example.org",
                "!a:example.org",
                "!c:example.org",
                "!b:example.org"
            ]
        );
        // By its latest activity, later than its last read.
        assert_eq!(l.open(&RoomId::new("!b:example.org")), Some(0));
        assert_eq!(
            ids(&l),
            [
                "!b:example.org",
                "!d:example.org",
                "!a:example.org",
                "!c:example.org"
            ]
        );
        // By its last read, later than its latest activity; !b goes back.
        assert_eq!(l.open(&RoomId::new("!c:example.org")), Some(0));
        assert_eq!(
            ids(&l),
            [
                "!c:example.org",
                "!d:example.org",
                "!a:example.org",
                "!b:example.org"
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
        assert_eq!(RoomSort::new(vec![Mentions]), Ok(RoomSort::one(Mentions)));
        assert!(RoomSort::new(vec![Mentions, Read]).is_ok());
        assert_eq!(RoomSort::default(), RoomSort::one(SortKey::Recent));
    }

    #[test]
    fn every_sort_key_has_a_name_of_its_own() {
        // In this order in the loader's warning for an unknown key.
        assert_eq!(
            SortKey::ALL.map(SortKey::config_name),
            ["mentions", "notify", "unread", "recent", "read", "name"]
        );
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
        assert_eq!(l.clear_unread(&a, None), Some((0, 1)));
        assert_eq!(
            ids(&l),
            ["!b:example.org", "!a:example.org", "!c:example.org"]
        );
        assert_eq!(l.get(&a).unwrap().unread, UnreadCounts::default());
        assert_eq!(
            l.clear_unread(&RoomId::new("!gone:example.org"), None),
            None
        );
    }

    #[test]
    fn clear_unread_puts_a_room_with_something_to_read_at_the_top_of_the_read_rooms() {
        let mut l = list(
            RoomSort::new(vec![SortKey::Unread, SortKey::Read]).unwrap(),
            vec![
                read_at(
                    room("!a:example.org", None, "a", Some(50), unread(1)),
                    Some(5),
                ),
                read_at(
                    room("!b:example.org", None, "b", Some(30), unread(2)),
                    Some(20),
                ),
                // Read after !a's newest message.
                read_at(
                    room("!c:example.org", None, "c", Some(40), unread(0)),
                    Some(60),
                ),
                // Nothing unread, with activity after the last receipt.
                read_at(
                    room("!d:example.org", None, "d", Some(70), unread(0)),
                    Some(10),
                ),
            ],
        );
        let at = |time| Some(Timestamp::from_millis(time));
        // Open, !d counts as read up to its latest activity.
        assert_eq!(l.open(&RoomId::new("!d:example.org")), Some(2));
        assert_eq!(
            ids(&l),
            [
                "!a:example.org",
                "!b:example.org",
                "!d:example.org",
                "!c:example.org"
            ]
        );
        // Above the open !d and !c: the echo will bring the homeserver's
        // time, later than both.
        let a = RoomId::new("!a:example.org");
        assert_eq!(l.clear_unread(&a, at(50)), Some((0, 1)));
        assert_eq!(l.get(&a).unwrap().last_read, at(71));
        let b = RoomId::new("!b:example.org");
        assert_eq!(l.clear_unread(&b, at(30)), Some((0, 0)));
        assert_eq!(l.get(&b).unwrap().last_read, at(72));
        assert_eq!(
            ids(&l),
            [
                "!b:example.org",
                "!a:example.org",
                "!d:example.org",
                "!c:example.org"
            ]
        );
        // A room whose latest activity is newer than every other time takes it.
        l.upsert(room("!e:example.org", None, "e", Some(100), unread(1)));
        let e = RoomId::new("!e:example.org");
        assert_eq!(l.clear_unread(&e, at(100)), Some((0, 0)));
        assert_eq!(l.get(&e).unwrap().last_read, at(100));
    }

    #[test]
    fn clear_unread_leaves_a_room_with_nothing_to_read_in_place() {
        let mut l = list(
            RoomSort::one(SortKey::Read),
            vec![
                read_at(
                    room("!a:example.org", None, "a", Some(30), unread(0)),
                    Some(30),
                ),
                // Its only news since its receipt is an edit, at 45, of a
                // message sent at 5.
                read_at(
                    room("!b:example.org", None, "b", Some(45), unread(0)),
                    Some(10),
                ),
                read_at(
                    room("!c:example.org", None, "c", Some(20), unread(0)),
                    Some(50),
                ),
            ],
        );
        let at = |time| Some(Timestamp::from_millis(time));
        let (a, b) = (RoomId::new("!a:example.org"), RoomId::new("!b:example.org"));
        // Receipts on events no newer than their last receipts (for !b the
        // edited message), which the homeserver ignores.
        assert_eq!(l.clear_unread(&b, at(5)), Some((2, 2)));
        assert_eq!(l.get(&b).unwrap().last_read, at(10));
        assert_eq!(l.clear_unread(&a, at(30)), Some((1, 1)));
        assert_eq!(l.get(&a).unwrap().last_read, at(30));
        // `room.mark_read`'s receipt goes on the edit, which is new.
        assert_eq!(l.clear_unread(&b, at(45)), Some((2, 0)));
        assert_eq!(l.get(&b).unwrap().last_read, at(51));
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
    fn upsert_never_lowers_the_last_read() {
        let mut l = list(
            RoomSort::one(SortKey::Read),
            vec![
                read_at(
                    room("!a:example.org", None, "a", Some(10), unread(0)),
                    Some(30),
                ),
                read_at(
                    room("!b:example.org", None, "b", Some(20), unread(0)),
                    Some(20),
                ),
            ],
        );
        let a = RoomId::new("!a:example.org");
        // Facts read before the receipt's echo: an older receipt, or none.
        for stale in [Some(5), None] {
            let update = read_at(
                room("!a:example.org", None, "a", Some(10), unread(0)),
                stale,
            );
            assert_eq!(l.upsert(update), (Some(0), 0));
            assert_eq!(
                l.get(&a).unwrap().last_read,
                Some(Timestamp::from_millis(30))
            );
        }
        // A newer receipt replaces the stored one.
        let update = read_at(
            room("!b:example.org", None, "b", Some(20), unread(0)),
            Some(40),
        );
        assert_eq!(l.upsert(update), (Some(1), 0));
        assert_eq!(
            l.get(&RoomId::new("!b:example.org")).unwrap().last_read,
            Some(Timestamp::from_millis(40))
        );
        assert_eq!(ids(&l), ["!b:example.org", "!a:example.org"]);
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
    fn pick_first_skips_the_current_room() {
        let l = three_unread();
        let a = RoomId::new("!a:example.org");
        let b = RoomId::new("!b:example.org");
        let c = RoomId::new("!c:example.org");
        let first = pick(RoomFilter::Unread, Pick::First);
        // The second match when the first is current, the first otherwise.
        assert_eq!(resolve_room(&l, Some(&c), &first), Some(a.clone()));
        assert_eq!(resolve_room(&l, Some(&a), &first), Some(c.clone()));
        assert_eq!(resolve_room(&l, Some(&b), &first), Some(c));
    }

    #[test]
    fn picks_return_none_when_only_current_matches() {
        let l = list(
            recent(),
            vec![
                room("!a:example.org", None, "a", Some(10), unread(1)),
                room("!b:example.org", None, "b", Some(20), unread(0)),
            ],
        );
        let a = RoomId::new("!a:example.org");
        for p in Pick::ALL {
            assert_eq!(
                resolve_room(&l, Some(&a), &pick(RoomFilter::Unread, p)),
                None,
                "{p:?}"
            );
        }
    }

    #[test]
    fn clear_unread_makes_the_next_pick_skip_the_room() {
        let mut l = three_unread();
        let c = RoomId::new("!c:example.org");
        l.clear_unread(&c, None);
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

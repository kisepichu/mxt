//! The room list: joined rooms in display order, room filters and the
//! resolution of `room.open` targets.

use std::cmp::Ordering;

use crate::fuzzy;
use crate::ids::RoomId;
use crate::room::{RoomName, RoomSummary, UnreadCounts};

/// The order of the room list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RoomSort {
    /// Latest activity, newest first; rooms without activity last; ties by the
    /// `Name` order.
    #[default]
    Recent,
    /// Case-insensitive by the full displayed name (`network / room`), which
    /// groups rooms by network; ties by room id.
    Name,
}

impl RoomSort {
    /// A total order: room ids are unique and break every tie.
    fn compare(self, a: &RoomSummary, b: &RoomSummary) -> Ordering {
        let by_name = || {
            lowercase(&a.name)
                .cmp(lowercase(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        };
        match self {
            // `None` sorts before `Some` and older before newer, so comparing
            // `b` with `a` puts the newest first and rooms without activity last.
            RoomSort::Recent => b.latest_activity.cmp(&a.latest_activity).then_with(by_name),
            RoomSort::Name => by_name(),
        }
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

    pub fn sort(&self) -> RoomSort {
        self.sort
    }

    /// Inserts or replaces the room with the same id and restores the order.
    /// Returns the room's new index.
    pub fn upsert(&mut self, summary: RoomSummary) -> usize {
        self.remove(&summary.id);
        let sort = self.sort;
        let index = self
            .rooms
            .partition_point(|room| sort.compare(room, &summary) == Ordering::Less);
        self.rooms.insert(index, summary);
        index
    }

    pub fn remove(&mut self, id: &RoomId) -> Option<RoomSummary> {
        let index = self.position(id)?;
        Some(self.rooms.remove(index))
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

    /// Optimistic clearing: sets the room's counts to zero (and `marked` to false).
    /// The order does not depend on counts, so it stays the same.
    pub fn clear_unread(&mut self, id: &RoomId) {
        if let Some(room) = self.rooms.iter_mut().find(|room| room.id == *id) {
            room.unread = UnreadCounts::default();
        }
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
        }
    }

    fn unread(messages: u64) -> UnreadCounts {
        UnreadCounts {
            messages,
            ..UnreadCounts::default()
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

    #[test]
    fn recent_order_is_newest_first_with_inactive_rooms_last() {
        let l = list(
            RoomSort::Recent,
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
            RoomSort::Recent,
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
            RoomSort::Name,
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
    fn upsert_replaces_and_reorders() {
        let mut l = list(
            RoomSort::Recent,
            vec![
                room("!a:example.org", None, "a", Some(10), unread(0)),
                room("!b:example.org", None, "b", Some(20), unread(0)),
            ],
        );
        let index = l.upsert(room("!a:example.org", None, "a", Some(30), unread(1)));
        assert_eq!(index, 0);
        assert_eq!(ids(&l), ["!a:example.org", "!b:example.org"]);
        assert_eq!(l.len(), 2);
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
            RoomSort::Recent,
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
            RoomSort::Recent,
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
            RoomSort::Recent,
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
            RoomSort::Recent,
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

//! Room summaries as the room list shows them: names, unread counts, badges
//! and notification modes, and the network name rule.

use std::fmt;

use crate::ids::{RoomId, Timestamp};

/// Between the network name and the room name in a displayed name.
const SEPARATOR: &str = " / ";

/// The displayed name of a room: `<network> / <room>`, or `<room>` alone when
/// the room has no network name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomName {
    /// The result of [`network_name`].
    pub network: Option<String>,
    /// The room's display name; the other user's display name for direct
    /// messages.
    pub room: String,
}

impl RoomName {
    /// The characters of the displayed name, without building the string.
    pub(crate) fn display_chars(&self) -> impl Iterator<Item = char> {
        let network = self.network.iter();
        let prefix = network.flat_map(|network| network.chars().chain(SEPARATOR.chars()));
        prefix.chain(self.room.chars())
    }
}

impl fmt::Display for RoomName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.network {
            Some(network) => write!(f, "{network}{SEPARATOR}{}", self.room),
            None => f.write_str(&self.room),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomKind {
    /// A direct message.
    Direct,
    Group,
}

/// The user-defined notification mode of a room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NotificationMode {
    /// No room rule: the account default applies.
    #[default]
    Default,
    MentionsAndKeywords,
    Mute,
}

impl NotificationMode {
    /// "Muted" for `Mute`, "Mentions only" for `MentionsAndKeywords`, else None.
    pub fn label(self) -> Option<&'static str> {
        match self {
            NotificationMode::Default => None,
            NotificationMode::MentionsAndKeywords => Some("Mentions only"),
            NotificationMode::Mute => Some("Muted"),
        }
    }
}

/// Unread counts as computed client-side by the SDK.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct UnreadCounts {
    /// Unread messages, counted even in muted rooms.
    pub messages: u64,
    /// Unread events that notify according to the user's push rules.
    pub notifications: u64,
    /// Unread highlights.
    pub mentions: u64,
    /// The manual unread mark.
    pub marked: bool,
}

/// A joined room as the room list shows it. Spaces are not room summaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomSummary {
    pub id: RoomId,
    pub name: RoomName,
    pub kind: RoomKind,
    pub unread: UnreadCounts,
    pub mode: NotificationMode,
    /// The time of the latest activity, if the SDK knows one.
    pub latest_activity: Option<Timestamp>,
}

/// The single unread badge of a room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    Mentions(u64),
    Notifications(u64),
    Messages(u64),
    Marked,
}

/// Counts above this show as `99+`.
const MAX_SHOWN_COUNT: u64 = 99;

impl Badge {
    /// First matching rule: mentions > 0, notifications > 0, messages > 0, marked.
    pub fn from_counts(counts: &UnreadCounts) -> Option<Badge> {
        if counts.mentions > 0 {
            Some(Badge::Mentions(counts.mentions))
        } else if counts.notifications > 0 {
            Some(Badge::Notifications(counts.notifications))
        } else if counts.messages > 0 {
            Some(Badge::Messages(counts.messages))
        } else if counts.marked {
            Some(Badge::Marked)
        } else {
            None
        }
    }

    /// "@3", "12", "7", "U"; counts above 99 show as "99+" ("@99+" for mentions).
    pub fn text(self) -> String {
        let (prefix, count) = match self {
            Badge::Mentions(count) => ("@", count),
            Badge::Notifications(count) | Badge::Messages(count) => ("", count),
            Badge::Marked => return "U".to_owned(),
        };
        if count > MAX_SHOWN_COUNT {
            format!("{prefix}{MAX_SHOWN_COUNT}+")
        } else {
            format!("{prefix}{count}")
        }
    }
}

/// Display names from a room's `m.bridge` (or `uk.half-shot.bridge`) event.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BridgeInfo {
    /// `network.displayname`.
    pub network: Option<String>,
    /// `protocol.displayname`.
    pub protocol: Option<String>,
}

/// The design's network name order; the first that yields a non-empty name wins:
/// bridge network, bridge protocol, canonical parent space name, the name of the
/// parent space when the user has joined exactly one, none.
///
/// Empty and whitespace-only values count as missing. The winning value is
/// returned trimmed.
///
/// `joined_parents` has one entry per parent space the user has joined,
/// including unnamed ones (pass `""`): the joined-parent step counts the
/// spaces, not their names.
pub fn network_name(
    bridge: Option<&BridgeInfo>,
    canonical_parent: Option<&str>,
    joined_parents: &[&str],
) -> Option<String> {
    let only_joined_parent = match joined_parents {
        [only] => Some(*only),
        _ => None,
    };
    [
        bridge.and_then(|b| b.network.as_deref()),
        bridge.and_then(|b| b.protocol.as_deref()),
        canonical_parent,
        only_joined_parent,
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|name| !name.is_empty())
    .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(messages: u64, notifications: u64, mentions: u64, marked: bool) -> UnreadCounts {
        UnreadCounts {
            messages,
            notifications,
            mentions,
            marked,
        }
    }

    #[test]
    fn badge_prefers_mentions_then_notifications_then_messages_then_marked() {
        assert_eq!(
            Badge::from_counts(&counts(9, 5, 2, true)),
            Some(Badge::Mentions(2))
        );
        assert_eq!(
            Badge::from_counts(&counts(9, 5, 0, true)),
            Some(Badge::Notifications(5))
        );
        assert_eq!(
            Badge::from_counts(&counts(9, 0, 0, true)),
            Some(Badge::Messages(9))
        );
        assert_eq!(
            Badge::from_counts(&counts(0, 0, 0, true)),
            Some(Badge::Marked)
        );
        assert_eq!(Badge::from_counts(&counts(0, 0, 0, false)), None);
    }

    #[test]
    fn badge_text_caps_at_99() {
        assert_eq!(Badge::Mentions(3).text(), "@3");
        assert_eq!(Badge::Mentions(100).text(), "@99+");
        assert_eq!(Badge::Notifications(99).text(), "99");
        assert_eq!(Badge::Messages(250).text(), "99+");
        assert_eq!(Badge::Marked.text(), "U");
    }

    #[test]
    fn mode_labels() {
        assert_eq!(NotificationMode::Mute.label(), Some("Muted"));
        assert_eq!(
            NotificationMode::MentionsAndKeywords.label(),
            Some("Mentions only")
        );
        assert_eq!(NotificationMode::Default.label(), None);
    }

    #[test]
    fn room_name_display() {
        let bridged = RoomName {
            network: Some("myserver".into()),
            room: "#general".into(),
        };
        assert_eq!(bridged.to_string(), "myserver / #general");
        let plain = RoomName {
            network: None,
            room: "Book club".into(),
        };
        assert_eq!(plain.to_string(), "Book club");
    }

    fn bridge(network: Option<&str>, protocol: Option<&str>) -> BridgeInfo {
        BridgeInfo {
            network: network.map(Into::into),
            protocol: protocol.map(Into::into),
        }
    }

    #[test]
    fn network_name_prefers_bridge_network_then_protocol() {
        let b = bridge(Some("myserver"), Some("Discord"));
        assert_eq!(
            network_name(Some(&b), Some("Space"), &["Space"]).as_deref(),
            Some("myserver")
        );
        // A Discord DM: the bridge sets only the protocol.
        let dm = bridge(None, Some("Discord"));
        assert_eq!(
            network_name(Some(&dm), None, &[]).as_deref(),
            Some("Discord")
        );
    }

    #[test]
    fn network_name_treats_empty_values_as_missing() {
        let b = bridge(Some("  "), Some(""));
        assert_eq!(
            network_name(Some(&b), Some("Space"), &[]).as_deref(),
            Some("Space")
        );
        assert_eq!(
            network_name(None, Some(" "), &["Only"]).as_deref(),
            Some("Only")
        );
    }

    #[test]
    fn network_name_trims_the_winning_value() {
        let b = bridge(Some(" Discord "), None);
        assert_eq!(
            network_name(Some(&b), None, &[]).as_deref(),
            Some("Discord")
        );
    }

    #[test]
    fn network_name_falls_back_to_parent_spaces() {
        assert_eq!(
            network_name(None, Some("Canonical"), &["Other"]).as_deref(),
            Some("Canonical")
        );
        assert_eq!(network_name(None, None, &["Only"]).as_deref(), Some("Only"));
        assert_eq!(network_name(None, None, &["One", "Two"]), None);
        assert_eq!(network_name(None, None, &[]), None);
    }
}

use super::timeline::{event, msg, status};
use super::*;
use crate::room::Badge;
use crate::rooms::{Pick, RoomFilter, RoomSort, RoomTarget, SortKey};
use crate::timeline::TimelineDiff;

const D: &str = "!d:example.org";
const C: &str = "!c:example.org";
const B: &str = "!b:example.org";
const A: &str = "!a:example.org";

/// A room of the `Work` network with `mentions` unread mentions.
fn named(rid: &str, name: &str, at: u64, mentions: u64) -> RoomSummary {
    RoomSummary {
        name: RoomName {
            network: Some("Work".into()),
            room: name.into(),
        },
        unread: UnreadCounts {
            messages: mentions,
            notifications: mentions,
            mentions,
            marked: false,
        },
        ..room(rid, at, 0)
    }
}
/// The room picker's items as (label, badge, room ID).
fn items(app: &App) -> Vec<(&str, Option<Badge>, &str)> {
    app.picker()
        .unwrap()
        .items
        .iter()
        .map(|item| match &item.key {
            PickerKey::Room(room) => (item.label.as_str(), item.badge, room.as_str()),
            key => panic!("not a room: {key:?}"),
        })
        .collect()
}
fn with_mode(summary: RoomSummary, mode: NotificationMode) -> RoomSummary {
    RoomSummary { mode, ..summary }
}
fn subscribe(room: &str) -> Effect {
    Effect::Matrix(MatrixEffect::SubscribeTimeline(id(room)))
}
fn unsubscribe(room: &str) -> Effect {
    Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id(room)))
}
fn set_mute(room: &str, muted: bool) -> Effect {
    Effect::Matrix(MatrixEffect::SetMute {
        room: id(room),
        muted,
    })
}
/// The rooms of `app()` under `sort = "unread"`: `!a` and `!c`, which are
/// unread, by name, then `!b`.
fn unread_first(user: Vec<(TableId, KeySeq, Binding)>) -> App {
    let settings = Settings {
        room_sort: RoomSort::one(SortKey::Unread),
        ..Settings::default()
    };
    app_in(settings, user)
}
/// The rooms of `app()` under `sort = "mentions, notify, unread, read"`:
/// `!c` notifies, `!a`, muted, has only a dim count, and `!b` is read.
fn notify_first(user: Vec<(TableId, KeySeq, Binding)>) -> App {
    let sort = vec![
        SortKey::Mentions,
        SortKey::Notify,
        SortKey::Unread,
        SortKey::Read,
    ];
    let settings = Settings {
        room_sort: RoomSort::new(sort).unwrap(),
        ..Settings::default()
    };
    let mut app = app_in(settings, user);
    app.update(Msg::RoomUpserted(with_mode(
        room(A, 10, 1),
        NotificationMode::Mute,
    )));
    app.update(Msg::RoomUpserted(RoomSummary {
        unread: UnreadCounts {
            messages: 2,
            notifications: 2,
            ..UnreadCounts::default()
        },
        ..room(C, 30, 2)
    }));
    assert_eq!(order(&app), [C, A, B]);
    app
}
fn order(app: &App) -> Vec<&str> {
    app.rooms()
        .rooms()
        .iter()
        .map(|room| room.id.as_str())
        .collect()
}

#[test]
fn the_room_picker_lists_rooms_in_order_with_badges_and_ranks_by_name() {
    let mut app = app();
    app.update(Msg::RoomUpserted(named(
        "!devops:example.org",
        "devops",
        50,
        0,
    )));
    app.update(Msg::RoomUpserted(named("!dev:example.org", "dev", 40, 0)));
    assert_eq!(no_timers(keys(&mut app, "<Space><Space>")), vec![]);
    assert_eq!(
        app.picker().unwrap().kind,
        PickerKind::Rooms {
            filter: RoomFilter::All
        }
    );
    assert_eq!(app.mode_label(None), "PICKER");
    assert_eq!(
        items(&app),
        [
            ("Work / devops", None, "!devops:example.org"),
            ("Work / dev", None, "!dev:example.org"),
            ("c:example.org", Some(Badge::Messages(2)), C),
            ("b:example.org", None, B),
            ("a:example.org", Some(Badge::Messages(1)), A),
        ]
    );
    // Ranked on the displayed name: the exact name before the longer one.
    keys(&mut app, "dev");
    assert_eq!(
        items(&app),
        [
            ("Work / dev", None, "!dev:example.org"),
            ("Work / devops", None, "!devops:example.org"),
        ]
    );
    assert_eq!(
        no_timers(keys(&mut app, "<C-n><CR>")),
        vec![subscribe("!devops:example.org")]
    );
    assert!(app.picker().is_none());
    assert_eq!(app.open_room(), Some(&id("!devops:example.org")));
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn two_rooms_with_the_same_name_are_both_reachable_in_the_room_picker() {
    let mut app = app();
    app.update(Msg::RoomUpserted(named(
        "!one:example.org",
        "general",
        50,
        0,
    )));
    app.update(Msg::RoomUpserted(named(
        "!two:example.org",
        "general",
        40,
        0,
    )));
    keys(&mut app, "<Space><Space>general");
    assert_eq!(
        items(&app),
        [
            ("Work / general", None, "!one:example.org"),
            ("Work / general", None, "!two:example.org"),
        ]
    );
    assert_eq!(
        no_timers(keys(&mut app, "<C-n><CR>")),
        vec![subscribe("!two:example.org")]
    );
    assert_eq!(
        no_timers(keys(&mut app, "<Space><Space>general<CR>")),
        vec![
            unsubscribe("!two:example.org"),
            subscribe("!one:example.org"),
        ]
    );
}

#[test]
fn the_mentions_filter_lists_only_rooms_with_mentions() {
    let mut app = app();
    keys(&mut app, "<Space>m");
    let picker = app.picker().unwrap();
    assert_eq!(
        picker.kind,
        PickerKind::Rooms {
            filter: RoomFilter::Mentions
        }
    );
    assert!(picker.items.is_empty());
    assert_eq!(picker.empty_text(), "No matching room");
    // Nothing to accept: the picker stays open.
    assert_eq!(no_timers(keys(&mut app, "<CR>")), vec![]);
    assert!(app.picker().is_some());
    keys(&mut app, "<Esc>");

    app.update(Msg::RoomUpserted(named("!ping:example.org", "ping", 5, 3)));
    keys(&mut app, "<Space>m");
    assert_eq!(
        items(&app),
        [("Work / ping", Some(Badge::Mentions(3)), "!ping:example.org")]
    );
    // The room went away while the picker was open.
    app.update(Msg::RoomRemoved(id("!ping:example.org")));
    assert_eq!(no_timers(keys(&mut app, "<CR>")), vec![]);
    assert!(app.picker().is_none());
    assert_eq!(status(&app), Some("No matching room"));
    assert_eq!(app.open_room(), None);
}

#[test]
fn mute_and_unmute_act_on_the_room_under_the_cursor_or_the_open_room() {
    let mut app = app();
    let muted = with_mode(room(C, 30, 2), NotificationMode::Mute);
    app.update(Msg::RoomUpserted(muted));
    keys(&mut app, "<CR><C-w>hj"); // open !c, the cursor on !b in the room list
    assert_eq!(
        no_timers(keys(&mut app, ":mute<CR>")),
        vec![set_mute(B, true)]
    );
    assert_eq!(app.status().unwrap().kind, StatusKind::Info);
    assert_eq!(status(&app), Some("Muted b:example.org"));
    // The label changes when the SDK reports the new mode.
    assert_eq!(
        app.rooms().get(&id(B)).unwrap().mode,
        NotificationMode::Default
    );
    keys(&mut app, "<C-w>l");
    assert_eq!(
        no_timers(keys(&mut app, ":unmute<CR>")),
        vec![set_mute(C, false)]
    );
    assert_eq!(status(&app), Some("Unmuted c:example.org"));
}

#[test]
fn unmute_leaves_a_room_that_is_not_muted_alone() {
    let mut app = app();
    // Set in another client; unmuting would remove the room's rule.
    let mentions_only = with_mode(room(C, 30, 2), NotificationMode::MentionsAndKeywords);
    app.update(Msg::RoomUpserted(mentions_only));
    assert_eq!(no_timers(keys(&mut app, ":unmute<CR>")), vec![]);
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    assert_eq!(status(&app), Some("c:example.org is not muted"));
    keys(&mut app, "j");
    assert_eq!(no_timers(keys(&mut app, ":unmute<CR>")), vec![]);
    assert_eq!(status(&app), Some("b:example.org is not muted"));
}

#[test]
fn mute_without_a_room_reports_it() {
    let mut app = app();
    keys(&mut app, "<C-w>l");
    assert_eq!(no_timers(keys(&mut app, ":mute<CR>")), vec![]);
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    assert_eq!(status(&app), Some("No room"));
}

#[test]
fn under_unread_the_cursor_stays_on_its_room_when_rooms_move() {
    let mut app = unread_first(vec![]);
    assert_eq!(order(&app), [A, C, B]);
    keys(&mut app, "G"); // on !b
    // A message in !b: the room under the cursor moves, the cursor along.
    app.update(Msg::RoomUpserted(room(B, 20, 1)));
    assert_eq!(order(&app), [A, B, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (B, Some(1)));
    // !a, read elsewhere, moves past the cursor's room.
    app.update(Msg::RoomUpserted(room(A, 10, 0)));
    assert_eq!(order(&app), [B, C, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (B, Some(0)));
}

#[test]
fn under_unread_the_open_room_sorts_as_read_until_another_room_opens() {
    let mut app = unread_first(vec![]);
    keys(&mut app, "G<CR>"); // open !b
    // A message in !b, whose count may come after its receipt: !b stays
    // among the read rooms, and its badge counts the message.
    app.update(Msg::RoomUpserted(room(B, 40, 1)));
    assert_eq!(order(&app), [A, C, B]);
    assert_eq!(app.rooms().get(&id(B)).unwrap().unread.messages, 1);
    assert_eq!((cursor_id(&app), app.room_cursor()), (B, Some(2)));
    // Opening !a: !b goes among the unread rooms by its counts, !a among
    // the read ones, and the cursor along with !a.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(B), subscribe(A)]
    );
    assert_eq!(order(&app), [B, C, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(2)));
    // With no room open, every room sorts by its counts.
    app.update(Msg::RoomRemoved(id(A)));
    app.update(Msg::RoomUpserted(room(A, 10, 1)));
    assert_eq!(order(&app), [A, B, C]);
}

#[test]
fn under_mentions_the_open_room_sorts_as_read_until_another_room_opens() {
    let settings = Settings {
        room_sort: RoomSort::one(SortKey::Mentions),
        ..Settings::default()
    };
    let mut app = app_in(settings, vec![]);
    assert_eq!(order(&app), [A, B, C]);
    keys(&mut app, "G<CR>"); // open !c
    let mentioned = RoomSummary {
        unread: UnreadCounts {
            messages: 3,
            notifications: 1,
            mentions: 1,
            marked: false,
        },
        ..room(C, 40, 3)
    };
    app.update(Msg::RoomUpserted(mentioned));
    assert_eq!(order(&app), [A, B, C]);
    assert_eq!(app.rooms().get(&id(C)).unwrap().unread.mentions, 1);
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(C), subscribe(A)]
    );
    assert_eq!(order(&app), [C, A, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
}

#[test]
fn under_notify_the_default_u_still_opens_a_room_whose_only_news_is_a_dim_count() {
    let mut app = notify_first(vec![]);
    assert_eq!(no_timers(keys(&mut app, "]u")), vec![subscribe(C)]);
    // !c sorts as read while open; the muted !a is the other unread room.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(C), subscribe(A)]
    );
}

#[test]
fn a_u_binding_with_filter_notify_passes_over_the_rooms_whose_only_news_is_a_dim_count() {
    let notify = Binding::single(Action::RoomOpen(RoomTarget::Pick {
        filter: RoomFilter::Notify,
        pick: Pick::First,
    }));
    let mut app = notify_first(vec![(TableId::Normal, seq("]u"), notify)]);
    assert_eq!(no_timers(keys(&mut app, "]u")), vec![subscribe(C)]);
    // !c's receipt comes back; only the muted !a is unread.
    app.update(Msg::RoomUpserted(room(C, 30, 0)));
    assert_eq!(no_timers(keys(&mut app, "]u")), vec![]);
    assert_eq!(status(&app), Some("No matching room"));
}

#[test]
fn under_recent_the_open_room_sorts_as_any_other() {
    let mut app = app();
    keys(&mut app, "j<CR>"); // open !b
    app.update(Msg::RoomUpserted(room(B, 40, 1)));
    assert_eq!(order(&app), [B, C, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (B, Some(0)));
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(B), subscribe(C)]
    );
    assert_eq!(order(&app), [B, C, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(1)));
}

#[test]
fn the_open_room_moves_among_the_read_rooms_with_the_cursor_and_u_follows() {
    let mark = Binding::single(Action::RoomMarkRead);
    let mut app = unread_first(vec![(TableId::Normal, seq("<Space>r"), mark)]);
    app.update(Msg::RoomUpserted(room(D, 40, 1)));
    assert_eq!(order(&app), [A, C, D, B]);
    keys(&mut app, "j<CR>"); // open !c, which sorts as read
    assert_eq!(order(&app), [A, D, B, C]);
    // Optimistic clearing leaves it there.
    assert_eq!(
        no_timers(keys(&mut app, "<Space>r")),
        vec![Effect::Matrix(MatrixEffect::MarkRoomRead(id(C)))]
    );
    assert_eq!(order(&app), [A, D, B, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(3)));
    // The next unread room after !c's new place, wrapping around; from its
    // old place it would have been !d.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(C), subscribe(A)]
    );

    // !a sorts as read once open too, and its receipt clears it.
    app.update(Msg::Timeline {
        timeline: main_of(A),
        diffs: vec![TimelineDiff::Reset(vec![msg(1)])],
    });
    let viewport = Msg::Viewport {
        pane: Pane::Timeline,
        first: 0,
        last: 0,
    };
    assert_eq!(
        app.update(viewport),
        vec![Effect::Matrix(MatrixEffect::SendReceipt {
            room: id(A),
            event: event(1),
        })]
    );
    assert_eq!(order(&app), [D, A, B, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(A), subscribe(D)]
    );
}

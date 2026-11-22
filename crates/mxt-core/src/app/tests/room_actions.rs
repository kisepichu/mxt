use super::threads::at;
use super::timeline::{event, msg, status};
use super::*;
use crate::action::ArgValue;
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
/// `summary` with the user's own latest read receipt at `at`.
fn read_at(summary: RoomSummary, at: u64) -> RoomSummary {
    RoomSummary {
        last_read: Some(Timestamp::from_millis(at)),
        ..summary
    }
}
/// `summary` with an unread mention, which notifies.
fn with_mention(summary: RoomSummary) -> RoomSummary {
    RoomSummary {
        unread: UnreadCounts {
            notifications: 1,
            mentions: 1,
            ..summary.unread
        },
        ..summary
    }
}
/// Under `sort = "mentions, unread, read"`: `rooms` over those of `app()`.
fn read_last(user: Vec<(TableId, KeySeq, Binding)>, rooms: Vec<RoomSummary>) -> App {
    let settings = Settings {
        room_sort: RoomSort::new(vec![SortKey::Mentions, SortKey::Unread, SortKey::Read]).unwrap(),
        ..Settings::default()
    };
    let mut app = app_in(settings, user);
    for summary in rooms {
        app.update(Msg::RoomUpserted(summary));
    }
    app
}
/// Shows the open `room`'s timeline, message `n` sent at `ms`, whole on the
/// pane; returns the effects of the viewport: the receipt, if any.
fn shown(app: &mut App, room: &str, n: u32, ms: u64) -> Vec<Effect> {
    app.update(Msg::Timeline {
        timeline: main_of(room),
        diffs: vec![TimelineDiff::Reset(vec![at(ms, msg(n))])],
    });
    app.update(Msg::Viewport {
        pane: Pane::Timeline,
        first: 0,
        last: 0,
    })
}
fn receipt(room: &str, n: u32) -> Effect {
    Effect::Matrix(MatrixEffect::SendReceipt {
        room: id(room),
        event: event(n),
    })
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
fn a_u_binding_with_filter_notify_skips_the_open_room_and_takes_a_new_mention() {
    let notify = Binding::single(
        Action::from_spec(
            "room.open",
            &[
                ("filter".into(), ArgValue::String("notify".into())),
                ("pick".into(), ArgValue::String("first".into())),
            ],
        )
        .unwrap(),
    );
    let mut app = notify_first(vec![(TableId::Normal, seq("]u"), notify)]);
    app.update(Msg::RoomUpserted(RoomSummary {
        unread: UnreadCounts {
            messages: 1,
            notifications: 1,
            ..UnreadCounts::default()
        },
        ..room(D, 40, 1)
    }));
    assert_eq!(order(&app), [D, C, A, B]);
    // No timeline is drawn, so no receipt goes out: the room opened last
    // still notifies, the next `]u` skips it while it is open, and it is
    // the topmost room that notifies again once another room is open.
    assert_eq!(no_timers(keys(&mut app, "]u")), vec![subscribe(D)]);
    assert_eq!(order(&app), [C, A, D, B]);
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(D), subscribe(C)]
    );
    assert_eq!(order(&app), [D, A, C, B]);
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(C), subscribe(D)]
    );
    // A mention in the muted !a puts it at the top, and `]u` takes it.
    app.update(Msg::RoomUpserted(with_mention(with_mode(
        room(A, 50, 1),
        NotificationMode::Mute,
    ))));
    assert_eq!(order(&app), [A, C, D, B]);
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(D), subscribe(A)]
    );
    // !c and !d read elsewhere: only the open room notifies.
    app.update(Msg::RoomUpserted(room(C, 30, 0)));
    app.update(Msg::RoomUpserted(room(D, 40, 0)));
    assert_eq!(no_timers(keys(&mut app, "]u")), vec![]);
    assert_eq!(status(&app), Some("No matching room"));
    assert_eq!(app.open_room(), Some(&id(A)));
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
fn under_unread_the_open_room_moves_among_the_read_rooms() {
    let mark = Binding::single(Action::RoomMarkRead);
    let mut app = unread_first(vec![(TableId::Normal, seq("<Space>r"), mark)]);
    app.update(Msg::RoomUpserted(room(D, 40, 1)));
    assert_eq!(order(&app), [A, C, D, B]);
    keys(&mut app, "j<CR>"); // open !c, which sorts as read
    assert_eq!(order(&app), [A, D, B, C]);
    // The cursor stays on its row, now on !d.
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(1)));
    // Optimistic clearing leaves it there.
    assert_eq!(
        no_timers(keys(&mut app, "<Space>r")),
        vec![Effect::Matrix(MatrixEffect::MarkRoomRead(id(C)))]
    );
    assert_eq!(order(&app), [A, D, B, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(1)));
    // The topmost unread room.
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

#[test]
fn under_read_a_room_marked_read_goes_to_the_top_of_the_read_rooms_at_once() {
    let mark = Binding::single(Action::RoomMarkRead);
    // !a got a message after a long silence; !d was read since, !b before.
    let mut app = read_last(
        vec![(TableId::Normal, seq("<Space>r"), mark)],
        vec![
            read_at(room(A, 50, 1), 5),
            read_at(room(B, 20, 0), 20),
            read_at(room(C, 30, 2), 25),
            read_at(room(D, 40, 0), 60),
        ],
    );
    assert_eq!(order(&app), [A, C, D, B]);
    // From the room list, its cursor on !a.
    assert_eq!(
        no_timers(keys(&mut app, "<Space>r")),
        vec![Effect::Matrix(MatrixEffect::MarkRoomRead(id(A)))]
    );
    // Above !d too: the receipt's echo brings the time the homeserver took
    // it, later than !d's.
    assert_eq!(order(&app), [C, A, D, B]);
    // Facts read before the echo, with the old receipt, leave it there, and
    // so does the echo.
    for read in [5, 90] {
        app.update(Msg::RoomUpserted(read_at(room(A, 50, 0), read)));
        assert_eq!(order(&app), [C, A, D, B]);
    }
}

#[test]
fn under_read_rooms_read_one_after_another_each_go_to_the_top_of_the_read_rooms() {
    // !d and !b were read after the newest messages of !a and !c.
    let mut app = read_last(
        vec![],
        vec![
            read_at(room(A, 50, 1), 5),
            read_at(room(B, 20, 0), 60),
            read_at(room(C, 30, 2), 25),
            read_at(room(D, 40, 0), 70),
        ],
    );
    assert_eq!(order(&app), [A, C, D, B]);
    assert_eq!(no_timers(keys(&mut app, "]u")), vec![subscribe(A)]);
    // Open, !a counts as read up to its newest message; its receipt then
    // puts it above the rooms read since.
    assert_eq!(order(&app), [C, D, B, A]);
    assert_eq!(shown(&mut app, A, 1, 50), vec![receipt(A, 1)]);
    assert_eq!(order(&app), [C, A, D, B]);
    // Facts read before the echo leave it there, and so does the echo.
    for read in [5, 100] {
        app.update(Msg::RoomUpserted(read_at(room(A, 50, 0), read)));
        assert_eq!(order(&app), [C, A, D, B]);
    }
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(A), subscribe(C)]
    );
    assert_eq!(order(&app), [A, D, B, C]);
    assert_eq!(shown(&mut app, C, 2, 30), vec![receipt(C, 2)]);
    assert_eq!(order(&app), [C, A, D, B]);
    for read in [25, 120] {
        app.update(Msg::RoomUpserted(read_at(room(C, 30, 0), read)));
        assert_eq!(order(&app), [C, A, D, B]);
    }
}

#[test]
fn under_read_a_room_whose_only_news_is_an_edit_goes_back_to_its_place() {
    // !b's only news since its receipt at 10 is an edit, at 45, of its
    // message sent at 8.
    let mut app = read_last(
        vec![],
        vec![
            read_at(room(A, 50, 1), 5),
            read_at(room(B, 45, 0), 10),
            read_at(room(C, 30, 2), 25),
            read_at(room(D, 40, 0), 40),
        ],
    );
    assert_eq!(order(&app), [A, C, D, B]);
    keys(&mut app, "G<CR>"); // open !b
    // Open, it counts as read up to the edit.
    assert_eq!(order(&app), [A, C, B, D]);
    // Its receipt goes on the edited message, which the homeserver
    // ignores: no echo will come.
    assert_eq!(shown(&mut app, B, 1, 8), vec![receipt(B, 1)]);
    assert_eq!(order(&app), [A, C, B, D]);
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(B), subscribe(A)]
    );
    assert_eq!(order(&app), [C, A, D, B]);
}

#[test]
fn the_default_u_opens_the_topmost_unread_room_other_than_the_open_one() {
    // Under "recent", the open room keeps its place among the others.
    let mut app = app();
    keys(&mut app, "j<CR>"); // open !b
    app.update(Msg::RoomUpserted(with_mention(room(D, 40, 1))));
    assert_eq!(order(&app), [D, C, B, A]);
    // The room with the new mention at the top, not !a after the open !b.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(B), subscribe(D)]
    );
    // The open room at the top is skipped.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(D), subscribe(C)]
    );
}

#[test]
fn under_read_the_default_u_opens_a_room_that_just_got_a_mention() {
    // Only !a is unread.
    let mut app = read_last(
        vec![],
        vec![
            read_at(room(A, 50, 1), 5),
            read_at(room(B, 20, 0), 20),
            read_at(room(C, 30, 0), 25),
            read_at(room(D, 40, 0), 60),
        ],
    );
    assert_eq!(order(&app), [A, D, C, B]);
    assert_eq!(no_timers(keys(&mut app, "]u")), vec![subscribe(A)]);
    assert_eq!(order(&app), [D, A, C, B]);
    // The open room is the only unread room.
    assert_eq!(no_timers(keys(&mut app, "]u")), vec![]);
    assert_eq!(status(&app), Some("No matching room"));
    // `]u` opened !a on the cursor's row, which kept the row, now on !d. A
    // mention in !b puts it at the top, and the cursor stays on !d.
    app.update(Msg::RoomUpserted(with_mention(read_at(room(B, 70, 1), 20))));
    assert_eq!(order(&app), [B, D, A, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(1)));
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(A), subscribe(B)]
    );
}

#[test]
fn the_default_u_alternates_between_the_two_topmost_unread_rooms_while_unfocused() {
    let mut app = read_last(
        vec![],
        vec![
            read_at(room(A, 50, 1), 5),
            read_at(room(B, 20, 0), 20),
            read_at(room(C, 30, 2), 25),
            read_at(room(D, 15, 1), 10),
        ],
    );
    assert_eq!(order(&app), [A, C, D, B]);
    assert_eq!(app.update(Msg::TerminalFocus(false)), vec![]);
    assert_eq!(no_timers(keys(&mut app, "]u")), vec![subscribe(A)]);
    // Without focus no receipt goes out, so !a stays unread; the next `]u`
    // skips it while it is open.
    assert_eq!(shown(&mut app, A, 1, 50), vec![]);
    assert_eq!(order(&app), [C, D, A, B]);
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(A), subscribe(C)]
    );
    // Once !c is open, !a is the topmost unread room again; !d never comes.
    assert_eq!(shown(&mut app, C, 2, 30), vec![]);
    assert_eq!(order(&app), [A, D, C, B]);
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(C), subscribe(A)]
    );
}

#[test]
fn the_default_u_backwards_opens_the_previous_unread_room_wrapping_around() {
    let mut app = app();
    app.update(Msg::RoomUpserted(room(D, 40, 1)));
    assert_eq!(order(&app), [D, C, B, A]);
    for expected in [A, C, D, A] {
        keys(&mut app, "[u");
        assert_eq!(app.open_room(), Some(&id(expected)), "{expected}");
    }
}

/// Under `sort = "mentions, unread, read"`: `!a`, `!d` and `!c`, unread,
/// newest message first, then `!b`, read at `b_read`.
fn inbox(user: Vec<(TableId, KeySeq, Binding)>, b_read: u64) -> App {
    read_last(
        user,
        vec![
            read_at(room(A, 50, 1), 5),
            read_at(room(B, 20, 0), b_read),
            read_at(room(C, 30, 2), 25),
            read_at(room(D, 40, 1), 10),
        ],
    )
}

#[test]
fn opening_the_room_under_the_cursor_leaves_the_cursor_on_its_row() {
    for open_d in ["<C-w>h<CR>", "]u", "<Space><Space><CR>", ":open d:ex<CR>"] {
        let mut app = inbox(vec![], 20);
        assert_eq!(order(&app), [A, D, C, B]);
        // `<CR>` on the topmost unread room: it moves among the read rooms,
        // and the next unread room takes its row under the cursor.
        assert_eq!(no_timers(keys(&mut app, "<CR>")), vec![subscribe(A)]);
        assert_eq!(order(&app), [D, C, A, B]);
        assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(0)));
        assert_eq!(shown(&mut app, A, 1, 50), vec![receipt(A, 1)]);
        // From the timeline, each way of opening the room under the cursor
        // does the same.
        keys(&mut app, open_d);
        assert_eq!(app.open_room(), Some(&id(D)), "{open_d}");
        assert_eq!(order(&app), [C, A, D, B], "{open_d}");
        assert_eq!(
            (cursor_id(&app), app.room_cursor()),
            (C, Some(0)),
            "{open_d}"
        );
    }
}

#[test]
fn opening_a_room_away_from_the_cursor_puts_the_cursor_on_it() {
    // !b was read after !a's newest message.
    let mut app = inbox(vec![], 60);
    // The last room, read, opens in place; the cursor stays in the list.
    keys(&mut app, "G<CR>");
    assert_eq!(order(&app), [A, D, C, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (B, Some(3)));
    // `]u` opens !a, away from the cursor, which goes to its new place.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(B), subscribe(A)]
    );
    assert_eq!(order(&app), [D, C, B, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(3)));
    // Its receipt lifts it to the top of the read rooms, and the cursor
    // goes along.
    assert_eq!(shown(&mut app, A, 1, 50), vec![receipt(A, 1)]);
    assert_eq!(order(&app), [D, C, A, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(2)));
}

/// Under `sort = "read"`: `!d`, read at 70, then `!a` and `!c`, unread, at
/// their newest messages, then `!b`, read at 20. `<Space>r` is
/// `room.mark_read`.
fn read_alone() -> App {
    let settings = Settings {
        room_sort: RoomSort::one(SortKey::Read),
        ..Settings::default()
    };
    let mark = Binding::single(Action::RoomMarkRead);
    let mut app = app_in(settings, vec![(TableId::Normal, seq("<Space>r"), mark)]);
    for summary in [
        read_at(room(A, 50, 1), 5),
        read_at(room(B, 20, 0), 20),
        read_at(room(C, 30, 2), 25),
        read_at(room(D, 40, 0), 70),
    ] {
        app.update(Msg::RoomUpserted(summary));
    }
    assert_eq!(order(&app), [D, A, C, B]);
    app
}

/// Reads the open `room`: by its receipt once its message `n`, sent at
/// `ms`, is shown, or by `room.mark_read`.
fn read_open(app: &mut App, by_receipt: bool, room: &str, n: u32, ms: u64) {
    if by_receipt {
        assert_eq!(shown(app, room, n, ms), vec![receipt(room, n)]);
    } else {
        let mark = Effect::Matrix(MatrixEffect::MarkRoomRead(id(room)));
        assert_eq!(no_timers(keys(app, "<Space>r")), vec![mark]);
    }
}

#[test]
fn reading_the_room_opened_under_the_cursor_leaves_the_cursor_on_its_row() {
    for by_receipt in [true, false] {
        let mut app = read_alone();
        // `<CR>` on !a: open, it counts as read up to its newest message,
        // so it keeps its row.
        assert_eq!(no_timers(keys(&mut app, "j<CR>")), vec![subscribe(A)]);
        assert_eq!(order(&app), [D, A, C, B]);
        assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
        // Read, it goes to the top, and !d takes its row; the rooms below
        // stay, so `j<CR>` opens the next one, as in a mail inbox.
        read_open(&mut app, by_receipt, A, 1, 50);
        assert_eq!(order(&app), [A, D, C, B], "{by_receipt}");
        assert_eq!(
            (cursor_id(&app), app.room_cursor()),
            (D, Some(1)),
            "{by_receipt}"
        );
        assert_eq!(
            no_timers(keys(&mut app, "<C-w>hj<CR>")),
            vec![unsubscribe(A), subscribe(C)],
            "{by_receipt}"
        );
        assert_eq!(order(&app), [A, D, C, B], "{by_receipt}");
        read_open(&mut app, by_receipt, C, 2, 30);
        assert_eq!(order(&app), [C, A, D, B], "{by_receipt}");
        assert_eq!(
            (cursor_id(&app), app.room_cursor()),
            (D, Some(2)),
            "{by_receipt}"
        );
        assert_eq!(
            no_timers(keys(&mut app, "<C-w>hj<CR>")),
            vec![unsubscribe(C), subscribe(B)],
            "{by_receipt}"
        );
    }
}

#[test]
fn reading_the_open_room_takes_the_cursor_along_once_the_cursor_moved() {
    for by_receipt in [true, false] {
        let mut app = read_alone();
        keys(&mut app, "j<CR>"); // open !a, under the cursor
        // Off !a and back onto it, then focus the timeline again.
        keys(&mut app, "<C-w>hjk<C-w>l");
        assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
        // Read, it goes to the top, and the cursor goes along.
        read_open(&mut app, by_receipt, A, 1, 50);
        assert_eq!(order(&app), [A, D, C, B], "{by_receipt}");
        assert_eq!(
            (cursor_id(&app), app.room_cursor()),
            (A, Some(0)),
            "{by_receipt}"
        );
    }
}

#[test]
fn an_update_of_the_open_room_leaves_the_cursor_on_its_row_only_when_it_was_read() {
    let renamed = |summary: RoomSummary| RoomSummary {
        name: RoomName {
            network: None,
            room: "renamed".into(),
        },
        ..summary
    };
    for (update, cursor) in [
        // Read: a newer receipt and cleared counts, as its echo brings.
        (read_at(room(A, 50, 0), 90), (D, Some(1))),
        // A new message.
        (read_at(room(A, 95, 2), 5), (A, Some(0))),
        // Read, and renamed or muted in the same update.
        (renamed(read_at(room(A, 50, 0), 90)), (A, Some(0))),
        (
            with_mode(read_at(room(A, 50, 0), 90), NotificationMode::Mute),
            (A, Some(0)),
        ),
    ] {
        let mut app = read_alone();
        keys(&mut app, "j<CR>"); // open !a, under the cursor
        assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
        app.update(Msg::RoomUpserted(update.clone()));
        assert_eq!(order(&app), [A, D, C, B], "{update:?}");
        assert_eq!((cursor_id(&app), app.room_cursor()), cursor, "{update:?}");
    }
    // Another room under the cursor, read elsewhere, takes the cursor along.
    let mut app = inbox(vec![], 60);
    keys(&mut app, "<CR>"); // open !a, the cursor on !d
    app.update(Msg::RoomUpserted(read_at(room(D, 40, 0), 100)));
    assert_eq!(order(&app), [C, D, B, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(1)));
}

#[test]
fn a_room_left_unread_comes_back_under_the_cursor_while_unfocused() {
    let mut app = inbox(vec![], 20);
    assert_eq!(app.update(Msg::TerminalFocus(false)), vec![]);
    // Without focus no receipt goes out, so !a stays unread.
    keys(&mut app, "<CR>");
    assert_eq!(shown(&mut app, A, 1, 50), vec![]);
    assert_eq!(order(&app), [D, C, A, B]);
    // Opening !d, which took the cursor's row, puts !a back on that row.
    assert_eq!(
        no_timers(keys(&mut app, "<C-w>h<CR>")),
        vec![unsubscribe(A), subscribe(D)]
    );
    assert_eq!(order(&app), [A, C, D, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(0)));
    // `<CR>` opens it again.
    assert_eq!(
        no_timers(keys(&mut app, "<C-w>h<CR>")),
        vec![unsubscribe(D), subscribe(A)]
    );
}

use super::threads::{at, in_the_thread};
use super::timeline::{event, msg, opened, status};
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
fn set_mode(room: &str, mode: NotificationMode) -> Effect {
    Effect::Matrix(MatrixEffect::SetMode {
        room: id(room),
        mode,
    })
}
fn mode_of(app: &App, room: &str) -> NotificationMode {
    app.rooms().get(&id(room)).unwrap().mode
}
/// `app()` with `!b`, read, in `mode` and the room list's cursor on it.
fn on_b(mode: NotificationMode) -> App {
    let mut app = app();
    app.update(Msg::RoomUpserted(with_mode(room(B, 20, 0), mode)));
    keys(&mut app, "j");
    assert_eq!(cursor_id(&app), B);
    app
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
        vec![set_mode(B, NotificationMode::Mute)]
    );
    assert_eq!(app.status().unwrap().kind, StatusKind::Info);
    assert_eq!(status(&app), Some("Muted b:example.org"));
    // The label changes at once, before the sync brings the new rules.
    assert_eq!(mode_of(&app, B).label(), Some("Muted"));
    keys(&mut app, "<C-w>l");
    assert_eq!(
        no_timers(keys(&mut app, ":unmute<CR>")),
        vec![set_mode(C, NotificationMode::Default)]
    );
    assert_eq!(status(&app), Some("Unmuted c:example.org"));
    assert_eq!(mode_of(&app, C), NotificationMode::Default);
}

#[test]
fn mute_then_unmute_before_the_echo_sends_both() {
    let mut app = app();
    // The cursor on !c, which has no rule of its own.
    assert_eq!(
        no_timers(keys(&mut app, ":mute<CR>")),
        vec![set_mode(C, NotificationMode::Mute)]
    );
    assert_eq!(mode_of(&app, C).label(), Some("Muted"));
    assert_eq!(
        no_timers(keys(&mut app, ":unmute<CR>")),
        vec![set_mode(C, NotificationMode::Default)]
    );
    assert_eq!(status(&app), Some("Unmuted c:example.org"));
    assert_eq!(mode_of(&app, C).label(), None);
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

    // Set with `:notify all` a moment ago: refused at once, before the echo.
    let mut app = on_b(NotificationMode::Default);
    assert_eq!(
        no_timers(keys(&mut app, ":notify all<CR>")),
        vec![set_mode(B, NotificationMode::AllMessages)]
    );
    assert_eq!(no_timers(keys(&mut app, ":unmute<CR>")), vec![]);
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    assert_eq!(status(&app), Some("b:example.org is not muted"));
    assert_eq!(mode_of(&app, B), NotificationMode::AllMessages);
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
fn notify_all_sets_the_room_under_the_cursor_at_once_and_moves_nothing() {
    let mut app = on_b(NotificationMode::MentionsAndKeywords);
    assert_eq!(
        no_timers(keys(&mut app, ":notify all<CR>")),
        vec![set_mode(B, NotificationMode::AllMessages)]
    );
    assert_eq!(app.status().unwrap().kind, StatusKind::Info);
    assert_eq!(
        status(&app),
        Some("b:example.org now notifies for every message")
    );
    assert_eq!(mode_of(&app, B), NotificationMode::AllMessages);
    assert_eq!(order(&app), [C, B, A]);
    assert_eq!(cursor_id(&app), B);

    let mut app = on_b(NotificationMode::AllMessages);
    assert_eq!(
        no_timers(keys(&mut app, ":notify mentions<CR>")),
        vec![set_mode(B, NotificationMode::MentionsAndKeywords)]
    );
    assert_eq!(
        status(&app),
        Some("b:example.org now notifies for mentions and keywords only")
    );
    assert_eq!(mode_of(&app, B), NotificationMode::MentionsAndKeywords);
}

#[test]
fn notify_acts_on_the_open_room_from_the_timeline_and_from_a_thread() {
    let mut app = opened();
    let mentions_only = with_mode(room(C, 30, 2), NotificationMode::MentionsAndKeywords);
    app.update(Msg::RoomUpserted(mentions_only));
    assert_eq!(
        no_timers(keys(&mut app, ":notify all<CR>")),
        vec![set_mode(C, NotificationMode::AllMessages)]
    );
    assert_eq!(
        status(&app),
        Some("c:example.org now notifies for every message")
    );

    let mut app = in_the_thread();
    assert_eq!(
        no_timers(keys(&mut app, ":notify mentions<CR>")),
        vec![set_mode(C, NotificationMode::MentionsAndKeywords)]
    );
    assert_eq!(
        status(&app),
        Some("c:example.org now notifies for mentions and keywords only")
    );
    assert_eq!(mode_of(&app, C), NotificationMode::MentionsAndKeywords);
}

#[test]
fn notify_with_the_rooms_own_mode_sends_nothing() {
    let mut app = on_b(NotificationMode::AllMessages);
    assert_eq!(no_timers(keys(&mut app, ":notify all<CR>")), vec![]);
    assert_eq!(app.status().unwrap().kind, StatusKind::Info);
    assert_eq!(
        status(&app),
        Some("b:example.org already notifies for every message")
    );

    let mut app = on_b(NotificationMode::MentionsAndKeywords);
    assert_eq!(no_timers(keys(&mut app, ":notify mentions<CR>")), vec![]);
    assert_eq!(
        status(&app),
        Some("b:example.org already notifies for mentions and keywords only")
    );
}

#[test]
fn notify_twice_before_the_echo_sends_one_change() {
    let mut app = on_b(NotificationMode::MentionsAndKeywords);
    let all = set_mode(B, NotificationMode::AllMessages);
    assert_eq!(no_timers(keys(&mut app, ":notify all<CR>")), vec![all]);
    assert_eq!(no_timers(keys(&mut app, ":notify all<CR>")), vec![]);
    assert_eq!(
        status(&app),
        Some("b:example.org already notifies for every message")
    );
}

#[test]
fn notify_all_then_mentions_before_the_echo_sends_both() {
    let mut app = on_b(NotificationMode::MentionsAndKeywords);
    assert_eq!(
        no_timers(keys(&mut app, ":notify all<CR>")),
        vec![set_mode(B, NotificationMode::AllMessages)]
    );
    assert_eq!(
        no_timers(keys(&mut app, ":notify mentions<CR>")),
        vec![set_mode(B, NotificationMode::MentionsAndKeywords)]
    );
    assert_eq!(
        status(&app),
        Some("b:example.org now notifies for mentions and keywords only")
    );
    assert_eq!(mode_of(&app, B), NotificationMode::MentionsAndKeywords);
}

#[test]
fn a_failed_change_puts_the_sdks_mode_back() {
    let mut app = on_b(NotificationMode::MentionsAndKeywords);
    let all = set_mode(B, NotificationMode::AllMessages);
    assert_eq!(
        no_timers(keys(&mut app, ":notify all<CR>")),
        vec![all.clone()]
    );
    // As the effect task answers a refused change.
    app.update(Msg::RoomModeChanged {
        room: id(B),
        mode: NotificationMode::MentionsAndKeywords,
    });
    let error = "Changing the room's notifications failed: forbidden";
    app.update(Msg::EffectFailed {
        error: error.into(),
    });
    assert_eq!(mode_of(&app, B), NotificationMode::MentionsAndKeywords);
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    assert_eq!(status(&app), Some(error));
    assert_eq!(no_timers(keys(&mut app, ":notify all<CR>")), vec![all]);
}

#[test]
fn notify_all_on_a_muted_room_sends_the_change_and_drops_the_label() {
    let mut app = on_b(NotificationMode::Mute);
    assert_eq!(
        no_timers(keys(&mut app, ":notify all<CR>")),
        vec![set_mode(B, NotificationMode::AllMessages)]
    );
    assert_eq!(mode_of(&app, B).label(), None);
}

#[test]
fn notify_alone_shows_the_rooms_setting() {
    let settings = [
        (
            NotificationMode::AllMessages,
            "b:example.org notifies for every message",
        ),
        (
            NotificationMode::MentionsAndKeywords,
            "b:example.org notifies for mentions and keywords only",
        ),
        (NotificationMode::Mute, "b:example.org is muted"),
        (
            NotificationMode::Default,
            "b:example.org follows the account's default",
        ),
    ];
    for (mode, text) in settings {
        let mut app = on_b(mode);
        assert_eq!(no_timers(keys(&mut app, ":notify<CR>")), vec![], "{mode:?}");
        assert_eq!(app.status().unwrap().kind, StatusKind::Info);
        assert_eq!(status(&app), Some(text));
        assert_eq!(mode_of(&app, B), mode);
    }
}

#[test]
fn notify_without_a_room_reports_it() {
    let mut app = app();
    keys(&mut app, "<C-w>l");
    for command in [":notify all<CR>", ":notify<CR>"] {
        assert_eq!(no_timers(keys(&mut app, command)), vec![]);
        assert_eq!(app.status().unwrap().kind, StatusKind::Error);
        assert_eq!(status(&app), Some("No room"));
    }
}

#[test]
fn notify_with_an_unknown_mode_sends_nothing() {
    let mut app = on_b(NotificationMode::MentionsAndKeywords);
    for mode in ["x", "All"] {
        let command = format!(":notify {mode}<CR>");
        assert_eq!(no_timers(keys(&mut app, &command)), vec![]);
        assert_eq!(app.status().unwrap().kind, StatusKind::Error);
        let error = format!("unknown mode \"{mode}\"; expected \"all\" or \"mentions\"");
        assert_eq!(status(&app), Some(error.as_str()));
    }
    assert_eq!(mode_of(&app, B), NotificationMode::MentionsAndKeywords);
}

#[test]
fn a_room_notify_binding_with_a_mode_sets_it() {
    let all = [("mode".into(), ArgValue::String("all".into()))];
    let notify = Binding::single(Action::from_spec("room.notify", &all).unwrap());
    let mut app = app_with(vec![(TableId::Normal, seq("<Space>n"), notify)]);
    assert_eq!(
        no_timers(keys(&mut app, "<Space>n")),
        vec![set_mode(C, NotificationMode::AllMessages)]
    );
    assert_eq!(
        status(&app),
        Some("c:example.org now notifies for every message")
    );
}

#[test]
fn a_mode_change_from_the_sdk_keeps_the_counts_and_the_place() {
    let mark = Binding::single(Action::RoomMarkRead);
    let mut app = unread_first(vec![(TableId::Normal, seq("<Space>r"), mark)]);
    assert_eq!(order(&app), [A, C, B]);
    // The cursor on !a, unread.
    keys(&mut app, "gg<Space>r");
    assert_eq!(order(&app), [C, A, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
    // Muted in another client, before the receipt's echo.
    app.update(Msg::RoomModeChanged {
        room: id(A),
        mode: NotificationMode::Mute,
    });
    assert_eq!(order(&app), [C, A, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
    let a = app.rooms().get(&id(A)).unwrap();
    assert_eq!(a.unread, UnreadCounts::default());
    assert_eq!(a.mode.label(), Some("Muted"));
    assert_eq!(
        no_timers(keys(&mut app, ":unmute<CR>")),
        vec![set_mode(A, NotificationMode::Default)]
    );
    // A room the list does not have is ignored.
    app.update(Msg::RoomModeChanged {
        room: id(D),
        mode: NotificationMode::Mute,
    });
    assert_eq!(order(&app), [C, A, B]);
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
    // the read ones, and the cursor to !c, the room after !a in the order
    // before the move.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(B), subscribe(A)]
    );
    assert_eq!(order(&app), [B, C, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(1)));
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
    // !c goes back to the top by its mention, which shifts !a down a row:
    // a change of !c, not a move of !a, so the cursor stays on !a.
    assert_eq!(order(&app), [C, A, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
    // A rename moves !a past !b, its first move: the cursor goes to !b,
    // the room after it.
    app.update(Msg::RoomUpserted(RoomSummary {
        name: RoomName {
            network: None,
            room: "z".into(),
        },
        ..room(A, 10, 1)
    }));
    assert_eq!(order(&app), [C, B, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (B, Some(1)));
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
    // The message moves !b to the top, its first move since `<CR>` put the
    // cursor on it: the cursor goes to !a, the room after it.
    assert_eq!(order(&app), [B, C, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(2)));
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
    // The cursor goes to !d, the room after it.
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(1)));
    // Optimistic clearing leaves it there.
    assert_eq!(
        no_timers(keys(&mut app, "<Space>r")),
        vec![Effect::Matrix(MatrixEffect::MarkRoomRead(id(C)))]
    );
    assert_eq!(order(&app), [A, D, B, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(1)));
    // The topmost unread room, which moves among the read rooms too: the
    // cursor goes to !d, the room after it.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(C), subscribe(A)]
    );
    assert_eq!(order(&app), [D, A, B, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(0)));

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
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(0)));
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
    // `]u` opened !a, which moved below !d, so the cursor went to !d. A
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

/// `summary` with its messages notifying.
fn notifying(summary: RoomSummary) -> RoomSummary {
    RoomSummary {
        unread: UnreadCounts {
            notifications: summary.unread.messages,
            ..summary.unread
        },
        ..summary
    }
}

/// Under the owner's `sort = "mentions, notify, unread, read"`: `!a` and
/// `!d` notify, newest message first, `!c` has a dim count, and `!b` is
/// read at `b_read`.
fn inbox(user: Vec<(TableId, KeySeq, Binding)>, b_read: u64) -> App {
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
    for summary in [
        read_at(notifying(room(A, 50, 1)), 5),
        read_at(room(B, 20, 0), b_read),
        read_at(room(C, 30, 2), 25),
        read_at(notifying(room(D, 40, 1)), 10),
    ] {
        app.update(Msg::RoomUpserted(summary));
    }
    assert_eq!(order(&app), [A, D, C, B]);
    app
}

#[test]
fn opening_a_room_that_moves_puts_the_cursor_on_the_room_after_it() {
    for open_d in ["<C-w>h<CR>", "]u", "<Space><Space><CR>", ":open d:ex<CR>"] {
        // !b was read after !a's newest message.
        let mut app = inbox(vec![], 60);
        // `<CR>` on the top room: it sorts as read as it opens and drops out
        // of the rooms that notify, and the cursor goes to !d, the room
        // after it, as in a mail inbox.
        assert_eq!(no_timers(keys(&mut app, "<CR>")), vec![subscribe(A)]);
        assert_eq!(order(&app), [D, C, B, A]);
        assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(0)));
        // Its receipt lifts it above !b, a second move: the cursor stays on
        // !d.
        assert_eq!(shown(&mut app, A, 1, 50), vec![receipt(A, 1)]);
        assert_eq!(order(&app), [D, C, A, B]);
        assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(0)));
        // Every way of opening !d does the same.
        keys(&mut app, open_d);
        assert_eq!(app.open_room(), Some(&id(D)), "{open_d}");
        assert_eq!(order(&app), [C, A, B, D], "{open_d}");
        assert_eq!(
            (cursor_id(&app), app.room_cursor()),
            (C, Some(0)),
            "{open_d}"
        );
    }
}

#[test]
fn the_owners_u_from_a_cursor_elsewhere_leaves_the_cursor_near_the_top() {
    let notify = Binding::single(Action::RoomOpen(RoomTarget::Pick {
        filter: RoomFilter::Notify,
        pick: Pick::First,
    }));
    let mut app = inbox(vec![(TableId::Normal, seq("]u"), notify)], 60);
    // `:open` puts the cursor on !b, read, which stays last.
    keys(&mut app, ":open b:ex<CR>");
    assert_eq!((cursor_id(&app), app.room_cursor()), (B, Some(3)));
    // `]u` opens !a, the top room that notifies, with the cursor on it. It
    // sorts as read and drops to the end, and the cursor goes to !d, the
    // room after it, the next one that notifies.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(B), subscribe(A)]
    );
    assert_eq!(order(&app), [D, C, B, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(0)));
    // Its receipt lifts it above !b; the cursor stays on !d, at the top.
    assert_eq!(shown(&mut app, A, 1, 50), vec![receipt(A, 1)]);
    assert_eq!(order(&app), [D, C, A, B]);
    keys(&mut app, "<C-w><C-h>");
    assert_eq!(app.focus(), Focus::RoomList);
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(0)));
    // `]u` opens !d, under the cursor, and the cursor goes to !c.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(A), subscribe(D)]
    );
    assert_eq!(order(&app), [C, A, B, D]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(0)));
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
fn reading_the_open_room_puts_the_cursor_on_the_room_that_was_below_it() {
    for by_receipt in [true, false] {
        let mut app = read_alone();
        // `<CR>` on !a: open, it counts as read up to its newest message,
        // so it stays on row 1.
        assert_eq!(no_timers(keys(&mut app, "j<CR>")), vec![subscribe(A)]);
        assert_eq!(order(&app), [D, A, C, B]);
        assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
        // Read, it goes to the top, and the cursor to !c, the room that was
        // below it, still on row 2, which `<CR>` opens.
        read_open(&mut app, by_receipt, A, 1, 50);
        assert_eq!(order(&app), [A, D, C, B], "{by_receipt}");
        assert_eq!(
            (cursor_id(&app), app.room_cursor()),
            (C, Some(2)),
            "{by_receipt}"
        );
        assert_eq!(
            no_timers(keys(&mut app, "<C-w>h<CR>")),
            vec![unsubscribe(A), subscribe(C)],
            "{by_receipt}"
        );
        assert_eq!(order(&app), [A, D, C, B], "{by_receipt}");
        read_open(&mut app, by_receipt, C, 2, 30);
        assert_eq!(order(&app), [C, A, D, B], "{by_receipt}");
        assert_eq!(
            (cursor_id(&app), app.room_cursor()),
            (B, Some(3)),
            "{by_receipt}"
        );
    }
}

#[test]
fn the_last_room_moving_puts_the_cursor_on_the_room_before_it() {
    let mut app = read_alone();
    keys(&mut app, "G<CR>"); // open !b, read, which stays last
    assert_eq!((cursor_id(&app), app.room_cursor()), (B, Some(3)));
    // A message lifts it to the top.
    app.update(Msg::RoomUpserted(read_at(room(B, 80, 1), 20)));
    assert_eq!(order(&app), [B, D, A, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(3)));
}

#[test]
fn a_room_added_above_the_open_room_leaves_the_cursor_on_it() {
    let e = "!e:example.org";
    let mut app = read_alone();
    keys(&mut app, "j<CR>"); // open !a, row 1
    // A room joined since, read later, goes to the top: a change of another
    // room, so the cursor stays on !a.
    app.update(Msg::RoomUpserted(read_at(room(e, 90, 0), 90)));
    assert_eq!(order(&app), [e, D, A, C, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(2)));
    // Its receipt is then its first move: the cursor goes to !c, the room
    // after it.
    read_open(&mut app, true, A, 1, 50);
    assert_eq!(order(&app), [A, e, D, C, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(3)));
}

#[test]
fn a_cursor_moved_off_the_open_room_before_it_moves_stays_on_its_room() {
    // Moving the cursor off !a and back onto it counts as moving it; only
    // moving the focus does not.
    for (moves, cursor) in [
        ("<C-w>h<C-w>l", (C, Some(2))),
        ("<C-w>hk<C-w>l", (D, Some(1))),
        ("<C-w>hjk<C-w>l", (A, Some(0))),
    ] {
        for by_receipt in [true, false] {
            let mut app = read_alone();
            keys(&mut app, "j<CR>"); // open !a
            keys(&mut app, moves);
            read_open(&mut app, by_receipt, A, 1, 50);
            assert_eq!(order(&app), [A, D, C, B], "{moves} {by_receipt}");
            assert_eq!(
                (cursor_id(&app), app.room_cursor()),
                cursor,
                "{moves} {by_receipt}"
            );
        }
    }
}

#[test]
fn any_first_move_of_the_open_room_puts_the_cursor_on_the_room_after_it() {
    // !a, open on row 1, read elsewhere (or its receipt's echo), or a new
    // message in it, lifts it to the top.
    for update in [read_at(room(A, 50, 0), 90), read_at(room(A, 95, 2), 5)] {
        let mut app = read_alone();
        keys(&mut app, "j<CR>");
        app.update(Msg::RoomUpserted(update.clone()));
        assert_eq!(order(&app), [A, D, C, B], "{update:?}");
        assert_eq!(
            (cursor_id(&app), app.room_cursor()),
            (C, Some(2)),
            "{update:?}"
        );
    }
    // A message in !b lifts it past !a, which shifts !a down a row: a
    // change of !b, not a move of !a, so the cursor stays on !a.
    let mut app = read_alone();
    keys(&mut app, "j<CR>");
    app.update(Msg::RoomUpserted(read_at(room(B, 80, 1), 20)));
    assert_eq!(order(&app), [B, D, A, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(2)));
    // Its receipt is then its first move: the cursor goes to !c, the room
    // after it.
    read_open(&mut app, true, A, 1, 50);
    assert_eq!(order(&app), [A, B, D, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(3)));
    // Another room under the cursor, read elsewhere, takes the cursor along.
    let mut app = inbox(vec![], 60);
    keys(&mut app, "<CR>"); // open !a, the cursor on !d
    app.update(Msg::RoomUpserted(read_at(room(D, 40, 0), 100)));
    assert_eq!(order(&app), [C, D, B, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(1)));
}

#[test]
fn rooms_removed_next_to_the_open_room_leave_the_cursor_on_it() {
    let mut app = read_alone();
    keys(&mut app, "jj<CR>"); // open !c, row 2
    // The room after it leaves, then the room above it, which shifts it up
    // a row: changes of other rooms, so the cursor stays on !c.
    app.update(Msg::RoomRemoved(id(B)));
    assert_eq!(order(&app), [D, A, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(2)));
    app.update(Msg::RoomRemoved(id(D)));
    assert_eq!(order(&app), [A, C]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(1)));
    // Its receipt is then its first move: it was last, so the cursor goes
    // to !a, the room before it.
    read_open(&mut app, true, C, 2, 30);
    assert_eq!(order(&app), [C, A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(1)));
    // The open room last: the room before it leaves, and the cursor stays
    // on it.
    keys(&mut app, "<C-w>h<CR>"); // open !a
    app.update(Msg::RoomRemoved(id(C)));
    assert_eq!(order(&app), [A]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (A, Some(0)));
}

#[test]
fn removing_the_open_room_puts_the_cursor_on_the_room_now_in_its_place() {
    let mut app = read_alone();
    keys(&mut app, "j<CR>"); // open !a, row 1
    app.update(Msg::RoomRemoved(id(A)));
    assert_eq!(app.open_room(), None);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(1)));
    // That room is not open, so its moves take the cursor along.
    app.update(Msg::RoomUpserted(room(C, 90, 3)));
    assert_eq!(order(&app), [C, D, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(0)));
    // The last room, open, leaves: the cursor goes to the new last room.
    keys(&mut app, "<C-w>hG<CR>"); // open !b
    app.update(Msg::RoomRemoved(id(B)));
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(1)));
    // The only room, open, leaves.
    keys(&mut app, "<C-w>h<CR>"); // open !d
    app.update(Msg::RoomRemoved(id(C)));
    app.update(Msg::RoomRemoved(id(D)));
    assert_eq!(app.room_cursor(), None);
}

#[test]
fn a_room_left_unread_goes_back_among_the_unread_rooms_while_unfocused() {
    let mut app = inbox(vec![], 20);
    assert_eq!(app.update(Msg::TerminalFocus(false)), vec![]);
    // Without focus no receipt goes out, so !a stays unread.
    keys(&mut app, "<CR>");
    assert_eq!(shown(&mut app, A, 1, 50), vec![]);
    assert_eq!(order(&app), [D, C, A, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (D, Some(0)));
    // Opening !d, under the cursor, puts !a back at the top, among the
    // rooms that notify, and !d among the read rooms: the cursor goes to
    // !c, the room after !d.
    assert_eq!(
        no_timers(keys(&mut app, "<C-w>h<CR>")),
        vec![unsubscribe(A), subscribe(D)]
    );
    assert_eq!(order(&app), [A, C, D, B]);
    assert_eq!((cursor_id(&app), app.room_cursor()), (C, Some(1)));
    // `]u` opens !a again.
    assert_eq!(
        no_timers(keys(&mut app, "]u")),
        vec![unsubscribe(D), subscribe(A)]
    );
}

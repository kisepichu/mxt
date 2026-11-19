use super::timeline::status;
use super::*;
use crate::rooms::RoomFilter;

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
/// The picker's items as (label, key).
fn items(app: &App) -> Vec<(&str, &str)> {
    app.picker()
        .unwrap()
        .items
        .iter()
        .map(|item| (item.label.as_str(), item.key.as_str()))
        .collect()
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
            ("Work / devops", "!devops:example.org"),
            ("Work / dev", "!dev:example.org"),
            ("c:example.org  2", C),
            ("b:example.org", B),
            ("a:example.org  1", A),
        ]
    );
    // Ranked on the displayed name: the exact name before the longer one.
    keys(&mut app, "dev");
    assert_eq!(
        items(&app),
        [
            ("Work / dev", "!dev:example.org"),
            ("Work / devops", "!devops:example.org"),
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
            ("Work / general", "!one:example.org"),
            ("Work / general", "!two:example.org"),
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
    assert_eq!(items(&app), [("Work / ping  @3", "!ping:example.org")]);
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
fn mute_without_a_room_reports_it() {
    let mut app = app();
    keys(&mut app, "<C-w>l");
    assert_eq!(no_timers(keys(&mut app, ":mute<CR>")), vec![]);
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    assert_eq!(status(&app), Some("No room"));
}

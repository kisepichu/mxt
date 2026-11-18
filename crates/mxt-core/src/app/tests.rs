use super::*;
use crate::action::{Action, Binding};
use crate::effect::{Effect, MatrixEffect, UiEffect};
use crate::ids::{RoomId, Timestamp};
use crate::keymap::{Keymap, TableId};
use crate::keys::KeySeq;
use crate::msg::{EditorMode, Msg, Pane};
use crate::room::{NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts};
use crate::settings::Settings;

fn space() -> KeySeq {
    KeySeq::parse("<Space>", &KeySeq::default()).unwrap()
}
fn seq(text: &str) -> KeySeq {
    KeySeq::parse(text, &space()).unwrap()
}
fn id(s: &str) -> RoomId {
    RoomId::new(s)
}
fn room(rid: &str, at: u64, messages: u64) -> RoomSummary {
    RoomSummary {
        id: id(rid),
        name: RoomName {
            network: None,
            room: rid.trim_start_matches('!').into(),
        },
        kind: RoomKind::Group,
        unread: UnreadCounts {
            messages,
            ..UnreadCounts::default()
        },
        mode: NotificationMode::Default,
        latest_activity: Some(Timestamp::from_millis(at)),
    }
}
fn app_with(user: Vec<(TableId, KeySeq, Binding)>) -> App {
    let mut app = App::new(Keymap::new(&space(), user), Settings::default());
    // Order (newest first): !c, !b, !a. !b is read.
    for r in [
        room("!a:example.org", 10, 1),
        room("!b:example.org", 20, 0),
        room("!c:example.org", 30, 2),
    ] {
        app.update(Msg::RoomUpserted(r));
    }
    app
}
fn app() -> App {
    app_with(vec![])
}
fn keys(app: &mut App, text: &str) -> Vec<Effect> {
    keys_in(app, text, None)
}
fn keys_in(app: &mut App, text: &str, editor: Option<EditorMode>) -> Vec<Effect> {
    seq(text)
        .keys()
        .iter()
        .flat_map(|&key| app.update(Msg::Key { key, editor }))
        .collect()
}
fn cursor_id(app: &App) -> &str {
    app.rooms().rooms()[app.room_cursor().unwrap()].id.as_str()
}
fn no_timers(effects: Vec<Effect>) -> Vec<Effect> {
    effects
        .into_iter()
        .filter(|e| !matches!(e, Effect::Ui(UiEffect::StartTimer { .. })))
        .collect()
}

#[test]
fn starts_on_the_room_list_with_the_cursor_on_the_first_room() {
    let app = app();
    assert_eq!(app.focus(), Focus::RoomList);
    assert_eq!(cursor_id(&app), "!c:example.org");
    assert_eq!(app.mode_label(None), "NORMAL");
}

#[test]
fn cursor_moves_with_counts_and_clamps() {
    let mut app = app();
    keys(&mut app, "j");
    assert_eq!(cursor_id(&app), "!b:example.org");
    keys(&mut app, "5j");
    assert_eq!(cursor_id(&app), "!a:example.org");
    keys(&mut app, "gg");
    assert_eq!(cursor_id(&app), "!c:example.org");
    keys(&mut app, "G");
    assert_eq!(cursor_id(&app), "!a:example.org");
}

#[test]
fn half_page_uses_the_last_viewport() {
    let mut app = app();
    for i in 0..20 {
        app.update(Msg::RoomUpserted(room(
            &format!("!r{i:02}:example.org"),
            100 + i,
            0,
        )));
    }
    keys(&mut app, "gg");
    app.update(Msg::Viewport {
        pane: Pane::RoomList,
        first: 0,
        last: 9,
    });
    keys(&mut app, "<C-d>");
    assert_eq!(app.room_cursor(), Some(5));
    keys(&mut app, "2<C-d>");
    assert_eq!(app.room_cursor(), Some(15));
}

#[test]
fn cursor_follows_its_room_when_the_order_changes() {
    let mut app = app();
    keys(&mut app, "j"); // on !b
    app.update(Msg::RoomUpserted(room("!b:example.org", 99, 1)));
    assert_eq!(cursor_id(&app), "!b:example.org");
    assert_eq!(app.room_cursor(), Some(0));
}

#[test]
fn cursor_stays_on_its_room_when_other_rooms_move_around_it() {
    let mut app = app();
    keys(&mut app, "j"); // on !b
    app.update(Msg::RoomUpserted(room("!d:example.org", 40, 0))); // new, above
    assert_eq!(cursor_id(&app), "!b:example.org");
    app.update(Msg::RoomUpserted(room("!c:example.org", 5, 0))); // from above to below
    assert_eq!(cursor_id(&app), "!b:example.org");
    app.update(Msg::RoomRemoved(id("!d:example.org"))); // above
    assert_eq!(cursor_id(&app), "!b:example.org");
    assert_eq!(app.room_cursor(), Some(0));
}

#[test]
fn opening_a_room_subscribes_and_focuses_the_timeline() {
    let mut app = app();
    let fx = keys(&mut app, "j<CR>");
    assert_eq!(
        fx,
        vec![Effect::Matrix(MatrixEffect::SubscribeTimeline(id(
            "!b:example.org"
        )))]
    );
    assert_eq!(app.open_room(), Some(&id("!b:example.org")));
    assert_eq!(app.focus(), Focus::Timeline);
    let fx = keys(&mut app, "<C-w>hgg<CR>");
    assert_eq!(
        no_timers(fx),
        vec![
            Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id("!b:example.org"))),
            Effect::Matrix(MatrixEffect::SubscribeTimeline(id("!c:example.org"))),
        ]
    );
}

#[test]
fn reopening_the_open_room_only_moves_focus() {
    let mut app = app();
    keys(&mut app, "<CR>");
    let fx = keys(&mut app, "<C-w>h<CR>");
    assert_eq!(no_timers(fx), vec![]);
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn open_next_unread_wraps_and_skips_read_rooms() {
    let next = Binding::single(
        Action::from_spec(
            "room.open",
            &[
                (
                    "filter".into(),
                    crate::action::ArgValue::String("unread".into()),
                ),
                (
                    "pick".into(),
                    crate::action::ArgValue::String("next".into()),
                ),
            ],
        )
        .unwrap(),
    );
    let mut app = app_with(vec![(TableId::Normal, seq("]u"), next)]);
    keys(&mut app, "]u");
    assert_eq!(app.open_room(), Some(&id("!c:example.org")));
    keys(&mut app, "]u");
    assert_eq!(app.open_room(), Some(&id("!a:example.org")));
    keys(&mut app, "]u");
    assert_eq!(app.open_room(), Some(&id("!c:example.org")));
}

#[test]
fn mark_read_then_next_unread_is_deterministic() {
    let next = Action::from_spec(
        "room.open",
        &[
            (
                "filter".into(),
                crate::action::ArgValue::String("unread".into()),
            ),
            (
                "pick".into(),
                crate::action::ArgValue::String("next".into()),
            ),
        ],
    )
    .unwrap();
    let both = Binding::new(vec![Action::RoomMarkRead, next]).unwrap();
    let mut app = app_with(vec![(TableId::Normal, seq("<Space>x"), both)]);
    keys(&mut app, "<CR>"); // open !c (unread 2)
    let fx = no_timers(keys(&mut app, "<Space>x"));
    assert_eq!(
        fx,
        vec![
            Effect::Matrix(MatrixEffect::MarkRoomRead(id("!c:example.org"))),
            Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id("!c:example.org"))),
            Effect::Matrix(MatrixEffect::SubscribeTimeline(id("!a:example.org"))),
        ]
    );
    assert_eq!(
        app.rooms().get(&id("!c:example.org")).unwrap().unread,
        UnreadCounts::default()
    );
}

#[test]
fn mark_read_targets_the_cursor_room_in_the_room_list() {
    let mut app = app_with(vec![(
        TableId::Normal,
        seq("<Space>r"),
        Binding::single(Action::RoomMarkRead),
    )]);
    keys(&mut app, "<CR><C-w>hG"); // open !c, back to the room list, cursor on !a
    let fx = no_timers(keys(&mut app, "<Space>r"));
    assert_eq!(
        fx,
        vec![Effect::Matrix(MatrixEffect::MarkRoomRead(id(
            "!a:example.org"
        )))]
    );
    keys(&mut app, "<C-w>l");
    let fx = no_timers(keys(&mut app, "<Space>r"));
    assert_eq!(
        fx,
        vec![Effect::Matrix(MatrixEffect::MarkRoomRead(id(
            "!c:example.org"
        )))]
    );
}

#[test]
fn sidebar_toggle_moves_focus_off_a_hidden_room_list() {
    let mut app = app();
    keys(&mut app, "<Space>e");
    assert!(!app.sidebar_visible());
    assert_eq!(app.focus(), Focus::Timeline);
    keys(&mut app, "<C-w>h");
    assert!(app.sidebar_visible());
    assert_eq!(app.focus(), Focus::RoomList);
}

#[test]
fn removing_the_open_room_unsubscribes_and_keeps_a_cursor() {
    let mut app = app();
    keys(&mut app, "<CR>"); // open !c
    keys(&mut app, "<C-w>h");
    let fx = app.update(Msg::RoomRemoved(id("!c:example.org")));
    assert_eq!(
        fx,
        vec![Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id(
            "!c:example.org"
        )))]
    );
    assert_eq!(app.open_room(), None);
    assert_eq!(cursor_id(&app), "!b:example.org");
}

#[test]
fn quit_and_unavailable_actions() {
    let mut app = app();
    assert_eq!(no_timers(keys(&mut app, "<Space><Space>")).len(), 0);
    assert_eq!(
        app.status().unwrap().text,
        "picker.rooms is not available yet"
    );
    let quit = Binding::single(Action::AppQuit);
    let mut app = app_with(vec![(TableId::Normal, seq("Q"), quit)]);
    assert_eq!(keys(&mut app, "Q"), vec![Effect::Ui(UiEffect::Quit)]);
}

#[test]
fn status_clears_on_the_next_key() {
    let mut app = app();
    app.update(Msg::EffectFailed {
        error: "boom".into(),
    });
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    keys(&mut app, "j");
    assert_eq!(app.status(), None);
}

#[test]
fn timeout_releases_through_the_original_context() {
    let mut app = app();
    let fx = keys(&mut app, "g");
    let Some(Effect::Ui(UiEffect::StartTimer { id, .. })) = fx.last().cloned() else {
        panic!("{fx:?}")
    };
    assert_eq!(app.pending_keys(), seq("g").keys());
    assert_eq!(app.update(Msg::Timeout { id }), vec![]);
    assert!(app.pending_keys().is_empty());
}

#[test]
fn editor_contexts_release_keys_to_the_editor() {
    let mut app = app();
    keys(&mut app, "<CR>");
    app.focus_composer_for_test();
    // Insert mode: unbound keys go to edtui; releases from one key are merged.
    let fx = no_timers(keys_in(&mut app, "<C-x>a", Some(EditorMode::Insert)));
    assert_eq!(
        fx,
        vec![Effect::Ui(UiEffect::ReplayKeys(
            seq("<C-x>a").keys().to_vec()
        ))]
    );
    // Normal mode: `x` is unbound in `composer.normal` and `normal`.
    assert_eq!(
        keys_in(&mut app, "x", Some(EditorMode::Normal)),
        vec![Effect::Ui(UiEffect::ReplayKeys(seq("x").keys().to_vec())),]
    );
    // Search mode: every key goes to edtui, even `<Esc>`.
    assert_eq!(
        keys_in(&mut app, "<Esc>", Some(EditorMode::Search)),
        vec![Effect::Ui(UiEffect::ReplayKeys(
            seq("<Esc>").keys().to_vec()
        )),]
    );
    assert_eq!(app.mode_label(Some(EditorMode::Insert)), "INSERT");
}

fn line(app: &App) -> Option<(LineKind, &str)> {
    app.command_line().map(|l| (l.kind, l.text.as_str()))
}

#[test]
fn command_line_runs_a_command() {
    let mut app = app();
    keys(&mut app, ":open b");
    assert_eq!(line(&app), Some((LineKind::Command, "open b")));
    assert_eq!(app.mode_label(None), "COMMAND");
    let fx = keys(&mut app, "<CR>");
    assert_eq!(
        fx,
        vec![Effect::Matrix(MatrixEffect::SubscribeTimeline(id(
            "!b:example.org"
        )))]
    );
    assert_eq!(line(&app), None);
}

#[test]
fn command_line_keys_do_not_trigger_bindings() {
    let mut app = app();
    keys(&mut app, ":jjgg");
    assert_eq!(line(&app), Some((LineKind::Command, "jjgg")));
    assert_eq!(cursor_id(&app), "!c:example.org");
}

#[test]
fn command_line_editing() {
    let mut app = app();
    keys(&mut app, ":open foo bar<C-w>");
    assert_eq!(line(&app), Some((LineKind::Command, "open foo ")));
    keys(&mut app, "<C-u>");
    assert_eq!(line(&app), Some((LineKind::Command, "")));
    keys(&mut app, "a<BS>");
    assert_eq!(line(&app), Some((LineKind::Command, "")));
    keys(&mut app, "<BS>");
    assert_eq!(line(&app), None);
}

#[test]
fn command_errors_go_to_the_status_line() {
    let mut app = app();
    keys(&mut app, ":quit<CR>");
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    // `:q` quits.
    assert_eq!(keys(&mut app, ":q<CR>"), vec![Effect::Ui(UiEffect::Quit)]);
}

#[test]
fn escape_cancels_the_command_line() {
    let mut app = app();
    keys(&mut app, ":open b<Esc>");
    assert_eq!(line(&app), None);
    assert_eq!(app.open_room(), None);
}

#[test]
fn completes_command_names_and_cycles_back() {
    let mut app = app();
    keys(&mut app, ":u");
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "unmute")));
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "upload ")));
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "u")));
}

#[test]
fn completes_room_names_for_open() {
    let mut app = app();
    keys(&mut app, ":open c<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "open c:example.org")));
}

#[test]
fn search_moves_the_cursor_and_repeats() {
    let mut app = app();
    keys(&mut app, "/A:<CR>");
    assert_eq!(cursor_id(&app), "!a:example.org");
    keys(&mut app, "n");
    assert_eq!(cursor_id(&app), "!a:example.org"); // only match: wraps onto itself
    keys(&mut app, "/example<CR>");
    assert_eq!(cursor_id(&app), "!c:example.org"); // after !a, wrapping
    keys(&mut app, "n");
    assert_eq!(cursor_id(&app), "!b:example.org");
    keys(&mut app, "N");
    assert_eq!(cursor_id(&app), "!c:example.org");
}

#[test]
fn search_failures() {
    let mut app = app();
    keys(&mut app, "n");
    assert_eq!(app.status().unwrap().text, "No previous search");
    keys(&mut app, "/zzz<CR>");
    assert_eq!(app.status().unwrap().text, "Pattern not found: zzz");
}

use super::*;
use crate::action::{Action, Binding};
use crate::effect::{Effect, MatrixEffect, RecoveryKey, UiEffect};
use crate::ids::{RoomId, TimelineId, Timestamp, UserId};
use crate::keymap::{Keymap, TableId};
use crate::keys::KeySeq;
use crate::msg::{EditorMode, Msg, Pane};
use crate::room::{NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts};
use crate::settings::Settings;

mod composer;
mod files;
mod history;
mod join;
mod jumps;
mod previews;
mod room_actions;
mod search;
mod threads;
mod timeline;

/// The user's own ID.
const ME: &str = "@me:example.org";

fn space() -> KeySeq {
    KeySeq::parse("<Space>", &KeySeq::default()).unwrap()
}
fn seq(text: &str) -> KeySeq {
    KeySeq::parse(text, &space()).unwrap()
}
fn id(s: &str) -> RoomId {
    RoomId::new(s)
}
/// The room's main timeline.
fn main_of(s: &str) -> TimelineId {
    TimelineId::main(id(s))
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
        last_read: None,
    }
}
fn app_with(user: Vec<(TableId, KeySeq, Binding)>) -> App {
    app_in(Settings::default(), user)
}
fn app_in(settings: Settings, user: Vec<(TableId, KeySeq, Binding)>) -> App {
    let mut app = App::new(Keymap::new(&space(), user), settings, UserId::new(ME));
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
/// Without `Effect::Store`: an executed `:` or `/` line saves the
/// histories, which the tests of `history.rs` check with
/// `keys_and_store`.
fn keys_in(app: &mut App, text: &str, editor: Option<EditorMode>) -> Vec<Effect> {
    let mut effects = keys_and_store(app, text, editor);
    effects.retain(|effect| !matches!(effect, Effect::Store(_)));
    effects
}
fn keys_and_store(app: &mut App, text: &str, editor: Option<EditorMode>) -> Vec<Effect> {
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
fn last_timer(effects: &[Effect]) -> TimerId {
    match effects.last() {
        Some(Effect::Ui(UiEffect::StartTimer { id, .. })) => *id,
        _ => panic!("{effects:?}"),
    }
}
/// The number of grapheme clusters on the recovery key prompt.
fn secret_len(app: &App) -> Option<usize> {
    app.secret_prompt().map(|secret| secret.clusters)
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
fn an_empty_room_list_ignores_cursor_and_room_keys() {
    let mark = Binding::single(Action::RoomMarkRead);
    let keymap = Keymap::new(&space(), vec![(TableId::RoomList, seq("m"), mark)]);
    let mut app = App::new(keymap, Settings::default(), UserId::new(ME));
    assert_eq!(keys(&mut app, "jkG<C-d><C-u><CR>"), vec![]);
    assert_eq!(app.room_cursor(), None);
    assert_eq!(app.open_room(), None);
    assert_eq!(keys(&mut app, "m"), vec![]);
    assert_eq!(app.status().unwrap().text, "No room");
}

#[test]
fn removing_rooms_at_the_end_clamps_then_unplaces_the_cursor() {
    let mut app = app();
    keys(&mut app, "G"); // on !a, the last room
    app.update(Msg::RoomRemoved(id("!a:example.org")));
    assert_eq!(cursor_id(&app), "!b:example.org");
    app.update(Msg::RoomRemoved(id("!b:example.org")));
    app.update(Msg::RoomRemoved(id("!c:example.org")));
    assert_eq!(app.room_cursor(), None);
    assert_eq!(keys(&mut app, "jG"), vec![]);
    // Unplaced again: the cursor is on whichever room is first.
    app.update(Msg::RoomUpserted(room("!d:example.org", 40, 0)));
    app.update(Msg::RoomUpserted(room("!e:example.org", 50, 0)));
    assert_eq!(cursor_id(&app), "!e:example.org");
}

#[test]
fn quit() {
    let quit = Binding::single(Action::AppQuit);
    let mut app = app_with(vec![(TableId::Normal, seq("Q"), quit)]);
    assert_eq!(keys(&mut app, "Q"), vec![Effect::Ui(UiEffect::Quit)]);
}

#[test]
fn recover_takes_the_key_masked_in_secret_mode_and_submits_it() {
    let mut app = app();
    assert_eq!(keys(&mut app, ":recover<CR>"), vec![]);
    assert_eq!(app.mode_label(None), "SECRET");
    assert_eq!(secret_len(&app), Some(0));

    // Keys bypass the keymap: `j` and `:` are text, `<BS>`, `<C-w>` and
    // `<C-u>` edit as on the command line, and other keys do nothing.
    let typed = keys(
        &mut app,
        "x<C-u>Ej:<Space>w<C-w>Abc<Space>dEf<BS>F<Tab><Up>",
    );
    assert_eq!(typed, vec![]);
    assert_eq!(app.command_line(), None);
    assert_eq!(cursor_id(&app), "!c:example.org");
    assert_eq!(secret_len(&app), Some(11));

    let effects = keys(&mut app, "<CR>");
    let key = RecoveryKey::from("Ej: Abc dEF".to_owned());
    assert_eq!(effects, vec![Effect::Matrix(MatrixEffect::Recover(key))]);
    let printed = format!("{effects:?}");
    assert!(!printed.contains("Abc"), "{printed}");
    assert_eq!(app.secret_prompt(), None);
    assert_eq!(app.mode_label(None), "NORMAL");
    assert_eq!(app.status().unwrap().text, "Recovering…");

    app.update(Msg::Recovered);
    let status = app.status().unwrap();
    assert_eq!(
        (status.kind, status.text.as_str()),
        (StatusKind::Info, "Recovery complete")
    );
}

#[test]
fn escape_cancels_the_recovery_and_a_blank_key_skips_it() {
    let mut app = app();
    assert_eq!(keys(&mut app, ":recover<CR>abc<Esc>"), vec![]);
    assert_eq!(app.secret_prompt(), None);
    assert_eq!(app.mode_label(None), "NORMAL");
    // The next prompt starts empty.
    keys(&mut app, ":recover<CR>");
    assert_eq!(secret_len(&app), Some(0));
    assert_eq!(keys(&mut app, "<Space><Space><CR>"), vec![]);
    assert_eq!(app.secret_prompt(), None);
    assert_eq!(app.status(), None);
    // The keymap applies again.
    keys(&mut app, "j");
    assert_eq!(cursor_id(&app), "!b:example.org");
}

#[test]
fn keys_typed_in_secret_mode_never_reach_the_composer() {
    let recover = Binding::single(Action::AccountRecover);
    let mut app = app_with(vec![(TableId::ComposerInsert, seq("<C-r>"), recover)]);
    keys(&mut app, "<CR>i");
    let insert = |app: &mut App, text: &str| keys_in(app, text, Some(EditorMode::Insert));
    assert_eq!(no_timers(insert(&mut app, "<C-r>")), vec![]);
    assert_eq!(app.mode_label(Some(EditorMode::Insert)), "SECRET");
    // Outside `SECRET` mode, insert mode replays these keys to the composer.
    for (n, key) in ["a", "j", ":", "<Space>", "Z"].into_iter().enumerate() {
        assert_eq!(insert(&mut app, key), vec![]);
        assert_eq!(secret_len(&app), Some(n + 1));
    }
    let key = RecoveryKey::from("aj: Z".to_owned());
    assert_eq!(
        insert(&mut app, "<CR>"),
        vec![Effect::Matrix(MatrixEffect::Recover(key))]
    );
    assert_eq!(app.mode_label(Some(EditorMode::Insert)), "INSERT");
}

#[test]
fn a_paste_in_secret_mode_never_reaches_the_composer() {
    let recover = Binding::single(Action::AccountRecover);
    let mut app = app_with(vec![
        (TableId::ComposerInsert, seq("<C-r>"), recover),
        (
            TableId::ComposerInsert,
            seq("<C-r>x"),
            Binding::single(Action::Nop),
        ),
    ]);
    keys(&mut app, "<CR>i");
    keys_in(&mut app, "<C-r>", Some(EditorMode::Insert));
    // The paste ends `<C-r>`, which enters `SECRET` mode, and goes into the
    // key, as keys typed after `<C-r>` would; so does the next paste, whose
    // line break goes.
    for (text, typed) in [("key", 3), ("key\n", 6)] {
        let pasted = app.update(Msg::Paste {
            text: text.into(),
            editor: Some(EditorMode::Insert),
        });
        assert_eq!(pasted, vec![]);
        assert_eq!(secret_len(&app), Some(typed));
    }
    let key = RecoveryKey::from("keykey".to_owned());
    assert_eq!(
        keys_in(&mut app, "<CR>", Some(EditorMode::Insert)),
        vec![Effect::Matrix(MatrixEffect::Recover(key))]
    );
}

#[test]
fn a_paste_in_secret_mode_goes_into_the_key_up_to_its_first_line_break() {
    // By default only `normal` opens the command line, with `:`; here
    // `<C-o>` opens it in the composer's insert mode and the message view.
    let user = || {
        vec![
            (
                TableId::ComposerInsert,
                seq("<C-o>"),
                Binding::single(Action::CommandLineOpen),
            ),
            (
                TableId::MessageView,
                seq("<C-o>"),
                Binding::single(Action::CommandLineOpen),
            ),
        ]
    };
    // Each opens the command line where the keys go and returns the mode of
    // the editor that has them, which `SECRET` mode then keeps.
    let setups: [fn(&mut App) -> Option<EditorMode>; 4] = [
        // The room list.
        |app| {
            keys(app, ":");
            None
        },
        // The composer in insert mode.
        |app| {
            keys(app, "<CR>i");
            keys_in(app, "<C-o>", Some(EditorMode::Insert));
            Some(EditorMode::Insert)
        },
        // The composer in normal mode.
        |app| {
            keys(app, "<CR>i");
            keys_in(app, "<Esc>", Some(EditorMode::Insert));
            keys_in(app, ":", Some(EditorMode::Normal));
            Some(EditorMode::Normal)
        },
        // The message view.
        |app| {
            keys(app, "<CR>");
            timeline::reset(app, vec![timeline::msg(1)]);
            keys(app, "<CR>");
            keys_in(app, "<C-o>", Some(EditorMode::Normal));
            Some(EditorMode::Normal)
        },
    ];
    for setup in setups {
        let mut app = app_with(user());
        let editor = setup(&mut app);
        assert_eq!(keys_in(&mut app, "recover<CR>", editor), vec![]);
        assert_eq!(secret_len(&app), Some(0));
        // A key copied with a line break and more after it: no binding runs,
        // nothing is submitted and no editor gets the text.
        let pasted = app.update(Msg::Paste {
            text: "EsTj 3yST\nrest".into(),
            editor,
        });
        assert_eq!(pasted, vec![]);
        assert_eq!(secret_len(&app), Some(9));
        let key = RecoveryKey::from("EsTj 3yST".to_owned());
        assert_eq!(
            keys_in(&mut app, "<CR>", editor),
            vec![Effect::Matrix(MatrixEffect::Recover(key))]
        );
    }
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
fn timeout_drops_a_pending_prefix_in_the_room_list() {
    let mut app = app();
    let id = last_timer(&keys(&mut app, "g"));
    assert_eq!(app.pending_keys(), seq("g").keys());
    assert_eq!(app.update(Msg::Timeout { id }), vec![]);
    assert!(app.pending_keys().is_empty());
}

/// `]` in the room list and `]gq` in `normal`, the bindings the comparison
/// with nvim 0.12.4 used.
fn brackets() -> App {
    app_with(vec![
        (
            TableId::RoomList,
            seq("]"),
            Binding::single(Action::CursorFirst),
        ),
        (
            TableId::Normal,
            seq("]gq"),
            Binding::single(Action::RoomMarkRead),
        ),
    ])
}

#[test]
fn a_mismatch_runs_the_longest_binding_and_resolves_the_rest_afresh() {
    let mut app = brackets();
    keys(&mut app, "G");
    // `]` runs, `gj` matches nothing, `g` is dropped and `j` runs.
    keys(&mut app, "]gj");
    assert_eq!(cursor_id(&app), "!b:example.org");
}

#[test]
fn a_timeout_runs_the_longest_binding_and_the_rest_waits_again() {
    let mut app = brackets();
    keys(&mut app, "G");
    let id = last_timer(&keys(&mut app, "]g"));
    let fx = app.update(Msg::Timeout { id });
    assert_eq!(cursor_id(&app), "!c:example.org");
    assert_eq!(app.pending_keys(), seq("g").keys());
    assert_ne!(last_timer(&fx), id);
}

#[test]
fn keys_left_after_a_binding_which_changes_the_context_go_to_the_new_context() {
    // `g` opens the command line and `gx` is only a prefix of `gxy`.
    let mut app = app_with(vec![
        (
            TableId::RoomList,
            seq("g"),
            Binding::single(Action::CommandLineOpen),
        ),
        (TableId::RoomList, seq("gxy"), Binding::single(Action::Nop)),
    ]);
    let id = last_timer(&keys(&mut app, "gx"));
    app.update(Msg::Timeout { id });
    assert_eq!(line(&app), Some((LineKind::Command, "x")));
    keys(&mut app, "<Esc>gxz");
    assert_eq!(line(&app), Some((LineKind::Command, "xz")));
}

#[test]
fn a_key_that_ends_a_binding_which_changes_the_context_goes_to_the_new_context() {
    // `g` is also a prefix of `gg`, so `j` ends it and `g` runs first.
    let open = Binding::new(vec![Action::RoomMute, Action::CommandLineOpen]).unwrap();
    let mut app = app_with(vec![(TableId::RoomList, seq("g"), open)]);
    keys(&mut app, "gj");
    assert_eq!(line(&app), Some((LineKind::Command, "j")));
    assert_eq!(cursor_id(&app), "!c:example.org");
    // The status the binding set survives the key that ended it.
    assert_eq!(app.status().unwrap().text, "Muted c:example.org");
}

#[test]
fn a_count_or_prefix_after_a_binding_which_changes_the_context_goes_to_the_new_context() {
    let open = Binding::single(Action::CommandLineOpen);
    let mut app = app_with(vec![(TableId::RoomList, seq("g"), open)]);
    // `1` would start a count in the room list.
    keys(&mut app, "g1");
    assert_eq!(line(&app), Some((LineKind::Command, "1")));
    assert_eq!(app.pending_count(), None);
    // `<C-w>` would start a timer for `<C-w>h` in the room list.
    keys(&mut app, "<Esc>g");
    assert_eq!(keys(&mut app, "<C-w>"), vec![]);
    assert!(app.pending_keys().is_empty());
    assert_eq!(line(&app), Some((LineKind::Command, "")));
}

#[test]
fn editor_contexts_release_keys_to_the_editor() {
    let mut app = app();
    keys(&mut app, "<CR>i");
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
    // The character after `f` or `r` goes to edtui, also `:` and `<Esc>`,
    // which `normal` and `composer.normal` bind.
    for key in [":", "<Esc>"] {
        assert_eq!(
            keys_in(&mut app, key, Some(EditorMode::AwaitingChar)),
            vec![Effect::Ui(UiEffect::ReplayKeys(seq(key).keys().to_vec()))]
        );
    }
    assert_eq!(app.focus(), Focus::Composer);
    assert_eq!(app.mode_label(Some(EditorMode::Insert)), "INSERT");
}

fn paste(app: &mut App, text: &str, editor: Option<EditorMode>) -> Vec<Effect> {
    no_timers(app.update(Msg::Paste {
        text: text.into(),
        editor,
    }))
}

#[test]
fn a_paste_goes_to_the_editor_as_text_and_never_sends() {
    let mut app = app();
    keys(&mut app, "<CR>i");
    // `<CR>` sends in insert mode; a pasted line break is text.
    for editor in [EditorMode::Insert, EditorMode::Normal] {
        assert_eq!(
            paste(&mut app, "one\ntwo\n", Some(editor)),
            vec![Effect::Ui(UiEffect::Paste("one\ntwo\n".into()))]
        );
    }
    assert_eq!(app.focus(), Focus::Composer);
}

#[test]
fn keys_pending_before_a_paste_end_first() {
    let mut app = app();
    keys(&mut app, "<CR>i");
    // `<C-x>` waits for `<C-x><C-e>`; it reaches the editor before the text.
    keys_in(&mut app, "<C-x>", Some(EditorMode::Insert));
    assert_eq!(
        paste(&mut app, "text", Some(EditorMode::Insert)),
        vec![
            Effect::Ui(UiEffect::ReplayKeys(seq("<C-x>").keys().to_vec())),
            Effect::Ui(UiEffect::Paste("text".into())),
        ]
    );
    assert!(app.pending_keys().is_empty());

    // Also once the editor waits for its next key, as edtui does after the
    // first `g` of `gg` while the second waits for `gt`.
    let gt = Binding::single(Action::FocusTimeline);
    let mut app = app_with(vec![(TableId::Normal, seq("gt"), gt)]);
    keys(&mut app, "<CR>i");
    keys_in(&mut app, "g", Some(EditorMode::Normal));
    assert_eq!(
        paste(&mut app, "text", Some(EditorMode::AwaitingChar)),
        vec![
            Effect::Ui(UiEffect::ReplayKeys(seq("g").keys().to_vec())),
            Effect::Ui(UiEffect::Paste("text".into())),
        ]
    );
    assert!(app.pending_keys().is_empty());
}

#[test]
fn the_command_line_and_pickers_take_the_first_line_of_a_paste() {
    let mut app = app();
    keys(&mut app, ":open ");
    assert_eq!(paste(&mut app, "b:example.org\n:q\n", None), vec![]);
    assert_eq!(line(&app), Some((LineKind::Command, "open b:example.org")));

    keys(&mut app, "<Esc><Space><Space>");
    assert_eq!(paste(&mut app, "b:ex\n<CR>", None), vec![]);
    let picker = app.picker().unwrap();
    assert_eq!(picker.query.text(), "b:ex");
    assert_eq!(picker.items[0].key, PickerKey::Room(id("!b:example.org")));
    // A paste that starts with a line break adds nothing.
    paste(&mut app, "\nmore", None);
    assert_eq!(app.picker().unwrap().query.text(), "b:ex");
}

#[test]
fn a_paste_in_the_room_list_or_the_timeline_does_nothing() {
    let mut app = app();
    // A count typed before it is dropped too.
    keys(&mut app, "2");
    assert_eq!(paste(&mut app, "j:q\n", None), vec![]);
    keys(&mut app, "j");
    assert_eq!(cursor_id(&app), "!b:example.org");
    keys(&mut app, "<CR><C-w>l");
    assert_eq!(paste(&mut app, "dd:q\n", None), vec![]);
    assert_eq!(app.focus(), Focus::Timeline);
    assert_eq!(line(&app), None);
    assert_eq!(app.confirmation(), None);
}

#[test]
fn a_paste_larger_than_256_kib_is_refused() {
    let mut app = app();
    keys(&mut app, "<CR>i");
    let limit = "x".repeat(256 * 1024);
    assert_eq!(
        paste(&mut app, &limit, Some(EditorMode::Insert)),
        vec![Effect::Ui(UiEffect::Paste(limit.clone()))]
    );
    let refused = |app: &App| {
        let status = app.status().unwrap();
        (status.kind, status.text.clone())
    };
    let too_large = (
        StatusKind::Info,
        "Paste too large (257 KiB; the limit is 256 KiB)".to_owned(),
    );
    let over = format!("{limit}y");
    assert_eq!(paste(&mut app, &over, Some(EditorMode::Insert)), vec![]);
    assert_eq!(refused(&app), too_large);

    // Also where only its first line would go, the recovery key too.
    let mut app = self::app();
    keys(&mut app, ":open ");
    assert_eq!(paste(&mut app, &format!("b\n{limit}"), None), vec![]);
    assert_eq!(line(&app), Some((LineKind::Command, "open ")));
    assert_eq!(refused(&app), too_large);
    keys(&mut app, "<Esc>:recover<CR>");
    assert_eq!(paste(&mut app, &format!("b\n{limit}"), None), vec![]);
    assert_eq!(secret_len(&app), Some(0));
    assert_eq!(refused(&app), too_large);
}

fn line(app: &App) -> Option<(LineKind, &str)> {
    app.command_line().map(|l| (l.kind, l.input.text()))
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
fn react_completes_with_a_space_before_recover() {
    let mut app = app();
    keys(&mut app, ":re<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "react ")));
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "recover")));
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "re")));
}

#[test]
fn completes_shortcodes_for_react_without_colons() {
    for typed in ["thu", ":thu"] {
        let mut app = app();
        keys(&mut app, &format!(":react {typed}"));
        for shown in ["react thumbsdown", "react thumbsup"] {
            keys(&mut app, "<Tab>");
            assert_eq!(line(&app), Some((LineKind::Command, shown)));
        }
        keys(&mut app, "<Tab>");
        let back = format!("react {typed}");
        assert_eq!(line(&app), Some((LineKind::Command, back.as_str())));
    }
    // Shortcodes are lower case.
    let mut app = app();
    keys(&mut app, ":react Thu<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "react Thu")));
}

#[test]
fn completes_room_names_for_open() {
    let mut app = app();
    keys(&mut app, ":open c<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "open c:example.org")));
}

#[test]
fn a_single_candidate_ends_the_completion() {
    let mut app = app();
    keys(&mut app, ":o<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "open ")));
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "open c:example.org")));
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "open b:example.org")));
}

#[test]
fn an_edit_after_tab_starts_a_new_completion() {
    let mut app = app();
    keys(&mut app, ":u<Tab><BS>");
    assert_eq!(line(&app), Some((LineKind::Command, "unmut")));
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "unmute")));
}

fn line_cursor(app: &App) -> usize {
    app.command_line().unwrap().input.cursor()
}

#[test]
fn the_command_and_search_lines_edit_at_the_cursor() {
    let mut app = app();
    keys(&mut app, ":open foo.bar<C-w><S-Left>x<End><BS><Home><Del>");
    assert_eq!(line(&app), Some((LineKind::Command, "pen foox")));
    assert_eq!(line_cursor(&app), 0);
    // `<BS>` at the start of a line with text does nothing; on an empty
    // line it closes the line.
    keys(&mut app, "<BS>");
    assert_eq!(line(&app), Some((LineKind::Command, "pen foox")));
    keys(&mut app, "<End><C-u>");
    assert_eq!(line(&app), Some((LineKind::Command, "")));
    keys(&mut app, "<BS>");
    assert_eq!(line(&app), None);

    keys(&mut app, "/exmple<Left><Left><Left><Left>a");
    assert_eq!(
        line(&app),
        Some((LineKind::Search(Pane::RoomList), "example"))
    );
    assert_eq!(line_cursor(&app), 3);
    keys(&mut app, "<CR>");
    assert_eq!(cursor_id(&app), "!b:example.org");
    keys(&mut app, "/<BS>");
    assert_eq!(line(&app), None);
}

#[test]
fn completion_reads_the_whole_line_and_leaves_the_cursor_at_the_end() {
    let mut app = app();
    keys(&mut app, ":u<Left>");
    assert_eq!(line_cursor(&app), 0);
    for shown in ["unmute", "upload ", "u", "unmute"] {
        keys(&mut app, "<Tab>");
        assert_eq!(line(&app), Some((LineKind::Command, shown)));
        assert_eq!(line_cursor(&app), shown.len());
    }
    // A cursor move ends the cycle: `<Tab>` completes `unmute`, its only
    // candidate, instead of showing `upload `.
    keys(&mut app, "<Left><Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "unmute")));
    assert_eq!(line_cursor(&app), 6);
}

#[test]
fn moving_in_a_picker_query_keeps_the_ranking_and_the_selection() {
    let mut app = app();
    keys(&mut app, "<Space><Space>example<C-n>");
    let ranked = app.picker().unwrap().items.clone();
    assert_eq!(ranked.len(), 3);
    // `<BS>` at the start moves and edits nothing, and keeps the picker.
    keys(&mut app, "<Left><Home><S-Right><C-b><BS><C-e><Left>");
    let picker = app.picker().unwrap();
    assert_eq!((picker.query.text(), picker.query.cursor()), ("example", 6));
    assert_eq!((&picker.items, picker.selected), (&ranked, 1));
    // Typing at the cursor ranks the items again and selects the first.
    keys(&mut app, "<Home>b:");
    let picker = app.picker().unwrap();
    assert_eq!(
        (picker.query.text(), picker.query.cursor()),
        ("b:example", 2)
    );
    assert_eq!(picker.items[0].key, PickerKey::Room(id("!b:example.org")));
    assert_eq!(picker.items.len(), 1);
    assert_eq!(picker.selected, 0);
}

#[test]
fn a_paste_goes_in_at_the_cursor() {
    let mut app = app();
    keys(&mut app, ":open .org<Home><S-Right><Right>");
    assert_eq!(paste(&mut app, "c:example\nrest", None), vec![]);
    assert_eq!(line(&app), Some((LineKind::Command, "open c:example.org")));
    assert_eq!(line_cursor(&app), "open c:example".len());

    keys(&mut app, "<Esc><Space><Space>.org<Home>");
    paste(&mut app, "b:example", None);
    let picker = app.picker().unwrap();
    assert_eq!(
        (picker.query.text(), picker.query.cursor()),
        ("b:example.org", 9)
    );
    assert_eq!(picker.items[0].key, PickerKey::Room(id("!b:example.org")));
    assert_eq!(picker.items.len(), 1);
}

#[test]
fn a_binding_in_picker_takes_a_line_editing_key_first() {
    let cancel = Binding::single(Action::PickerCancel);
    let mut app = app_with(vec![(TableId::Picker, seq("<C-e>"), cancel)]);
    keys(&mut app, "<Space><Space>ab<Home><C-e>");
    assert!(app.picker().is_none());
    // Without it, `<C-e>` moves the cursor to the end.
    let mut app = self::app();
    keys(&mut app, "<Space><Space>ab<Home><C-e>");
    assert_eq!(app.picker().unwrap().query.cursor(), 2);
}

#[test]
fn the_recovery_key_is_edited_at_its_cursor_in_its_own_buffer() {
    let mut app = app();
    keys(&mut app, ":recover<CR>");
    // A recovery key of 59 characters with a typo in its second group.
    let typo = "EsTa bcdX fghi jkmn opqr stuv wxyz ABCD EFGH JKLM NPQR STUV";
    paste(&mut app, typo, None);
    keys(&mut app, "<Home><S-Right><S-Right><BS>e");
    let prompt = app.secret_prompt().unwrap();
    assert_eq!((prompt.clusters, prompt.cursor), (59, 9));
    let effects = keys(&mut app, "<CR>");
    let [Effect::Matrix(MatrixEffect::Recover(key))] = &effects[..] else {
        panic!("{effects:?}");
    };
    let fixed = typo.replace('X', "e");
    assert_eq!(key.expose(), fixed);
    // The 128 bytes `start_recovery` allocated, never reallocated.
    let buffer: &String = std::borrow::Borrow::borrow(key);
    assert_eq!(buffer.capacity(), 128);

    // One `*` per grapheme cluster, the cursor among them.
    keys(&mut app, ":recover<CR>");
    paste(&mut app, "pass 👨\u{200d}👩\u{200d}👧 word", None);
    keys(&mut app, "<S-Left><S-Left>");
    let prompt = app.secret_prompt().unwrap();
    assert_eq!(
        (prompt.prompt, prompt.clusters, prompt.cursor),
        ("Recovery key: ", 11, 5)
    );
    keys(&mut app, "<BS><Del>");
    let prompt = app.secret_prompt().unwrap();
    assert_eq!((prompt.clusters, prompt.cursor), (9, 4));
    // `<BS>` at the start, also of an empty key, keeps the prompt.
    keys(&mut app, "<C-u><End><C-u><BS>");
    let prompt = app.secret_prompt().unwrap();
    assert_eq!((prompt.clusters, prompt.cursor), (0, 0));
}

/// A help view's text, which has no matches.
fn text_view(text: String) -> Effect {
    Effect::Ui(UiEffect::OpenView(crate::effect::ViewContent::Text {
        text,
        matches: Vec::new(),
    }))
}

#[test]
fn help_and_map_open_the_view_and_q_or_escape_close_it() {
    // `:` cannot be typed in a view, so `K` runs `help.keymap` there.
    let map = Binding::single(Action::HelpKeymap);
    let mut app = app_with(vec![(TableId::MessageView, seq("K"), map)]);
    let help = text_view(help_text(COMMANDS));
    let map = text_view(keymap_text(&app.keymap));
    let view = |app: &mut App, text: &str| keys_in(app, text, Some(EditorMode::Normal));

    assert_eq!(keys(&mut app, ":help<CR>"), vec![help]);
    assert!(app.view_open());
    assert_eq!(app.mode_label(Some(EditorMode::Normal)), "VIEW NORMAL");
    // A help action replaces the open view.
    assert_eq!(view(&mut app, "K"), vec![map.clone()]);
    assert!(app.view_open());
    assert_eq!(view(&mut app, "q"), vec![Effect::Ui(UiEffect::CloseView)]);
    assert!(!app.view_open());
    assert_eq!(app.focus(), Focus::RoomList);

    assert_eq!(keys(&mut app, ":map<CR>"), vec![map]);
    assert_eq!(
        view(&mut app, "<Esc>"),
        vec![Effect::Ui(UiEffect::CloseView)]
    );
    assert!(!app.view_open());

    // Unlike a message view, a help view stays when the open room goes.
    keys(&mut app, "<CR>:help<CR>");
    assert_eq!(
        app.update(Msg::RoomRemoved(id("!c:example.org"))),
        vec![Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id(
            "!c:example.org"
        )))]
    );
    assert!(app.view_open());
}

const MESSAGES_FIRST_LINE: &str = "Status line messages, oldest first; the last 200 are kept.";

/// The lines of the view `:messages` opens.
fn messages(app: &mut App) -> Vec<String> {
    match no_timers(keys(app, ":messages<CR>")).as_slice() {
        [Effect::Ui(UiEffect::OpenView(crate::effect::ViewContent::Text { text, .. }))] => {
            text.lines().map(str::to_owned).collect()
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn messages_lists_the_last_200_status_messages_oldest_first() {
    let mut app = app();
    assert_eq!(messages(&mut app), [MESSAGES_FIRST_LINE]);
    keys_in(&mut app, "q", Some(EditorMode::Normal));
    for n in 0..=200 {
        app.update(Msg::EffectFailed {
            error: format!("failure {n}"),
        });
    }
    let lines = messages(&mut app);
    assert_eq!(lines[..2], [MESSAGES_FIRST_LINE, ""]);
    let kept: Vec<String> = (1..=200).map(|n| format!("failure {n}")).collect();
    assert_eq!(lines[2..], kept);
}

#[test]
fn configuration_warnings_come_before_the_status_that_counts_them() {
    let warnings = [
        "config.toml:4:1: ui.font: unknown key (ignored)",
        "config.toml:6:1: notifications: unknown table (ignored)",
    ];
    // The status line and the lines of `:messages` after its first two.
    let start = |warnings: &[&str]| {
        let mut app = app();
        app.update(Msg::ConfigWarnings {
            warnings: warnings.iter().map(|&warning| warning.to_owned()).collect(),
        });
        let status = app.status().cloned();
        (status, messages(&mut app).split_off(2))
    };
    let error = |text: &str| StatusMessage {
        kind: StatusKind::Error,
        text: text.to_owned(),
    };

    let counted = "Ignored 2 settings in the configuration; see :messages";
    assert_eq!(
        start(&warnings),
        (
            Some(error(counted)),
            vec![warnings[0].into(), warnings[1].into(), counted.into()]
        )
    );
    let counted = "Ignored 1 setting in the configuration; see :messages";
    assert_eq!(
        start(&warnings[..1]),
        (
            Some(error(counted)),
            vec![warnings[0].into(), counted.into()]
        )
    );

    let mut app = app();
    app.update(Msg::ConfigWarnings { warnings: vec![] });
    assert_eq!(app.status(), None);
    assert_eq!(messages(&mut app), [MESSAGES_FIRST_LINE]);
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

use std::collections::HashSet;

use super::timeline::{event, item, local, msg, opened, remote, reset, status};
use super::*;
use crate::action::{FocusMode, ReactWith};
use crate::effect::{Outgoing, OutgoingKind};
use crate::emoji::{self, Emoji};
use crate::ids::{SendId, TxnId, UserId};
use crate::mention::{Member, MentionRef, RenderedBody, render_mentions};
use crate::rich::{Block, Inline, MentionTarget, RichText, Style};
use crate::rooms::{Pick, RoomFilter, RoomTarget};
use crate::timeline::{
    Draft, ItemRef, MessageContent, ReactionGroup, SendState, TimelineDiff, TimelineEntry,
    TimelineItem,
};

const C: &str = "!c:example.org";
const B: &str = "!b:example.org";
const A: &str = "!a:example.org";
const ALICE: &str = "@alice:example.org";

/// Changes the item of `entry`.
fn with(mut entry: TimelineEntry, change: impl FnOnce(&mut TimelineItem)) -> TimelineEntry {
    if let TimelineEntry::Item(item) = &mut entry {
        change(item);
    }
    entry
}
/// An item sent by the user.
fn own(id: ItemRef, body: &str) -> TimelineEntry {
    with(item(id, body), |item| item.sender = UserId::new(ME))
}
fn insert(app: &mut App, text: &str) -> Vec<Effect> {
    no_timers(keys_in(app, text, Some(EditorMode::Insert)))
}
fn normal(app: &mut App, text: &str) -> Vec<Effect> {
    no_timers(keys_in(app, text, Some(EditorMode::Normal)))
}
fn ui(effect: UiEffect) -> Effect {
    Effect::Ui(effect)
}
fn matrix(effect: MatrixEffect) -> Effect {
    Effect::Matrix(effect)
}
fn focus(room: &str) -> Effect {
    ui(UiEffect::FocusComposer {
        timeline: main_of(room),
        mode: FocusMode::Insert,
    })
}
fn load_members(room: &str) -> Effect {
    matrix(MatrixEffect::LoadMembers(id(room)))
}
fn clear(room: &str) -> Effect {
    ui(UiEffect::ClearComposer(main_of(room)))
}
/// The send of `body` from `!c`, the app's `n`-th from 0.
fn send(n: u64, body: RenderedBody, kind: OutgoingKind) -> Effect {
    matrix(MatrixEffect::Send {
        timeline: main_of(C),
        id: SendId(n),
        message: Outgoing { body, kind },
    })
}
fn plain(text: &str) -> RenderedBody {
    render_mentions(text, &[])
}
fn submit(app: &mut App, body: &str) -> Vec<Effect> {
    app.update(Msg::Submit {
        timeline: main_of(C),
        body: body.into(),
    })
}
fn edited(app: &mut App, room: &str, empty: bool) {
    app.update(Msg::ComposerEdited {
        timeline: main_of(room),
        empty,
    });
}
fn member(user: &str, name: &str) -> Member {
    Member {
        user: UserId::new(user),
        display_name: Some(name.into()),
    }
}
fn members(app: &mut App, members: Vec<Member>) {
    app.update(Msg::MembersLoaded {
        room: id(C),
        members,
    });
}
fn query(app: &mut App, query: Option<&str>) {
    app.update(Msg::MentionQuery {
        timeline: main_of(C),
        query: query.map(Into::into),
    });
}
fn menu(app: &App) -> Option<(Vec<&str>, usize)> {
    app.mention_menu().map(|(candidates, selected)| {
        let ids = candidates.iter().map(|m| m.user.as_str()).collect();
        (ids, selected)
    })
}
fn edit_target<'a>(app: &'a App, room: &str) -> Option<&'a ItemRef> {
    match app.draft(&main_of(room)) {
        Some(Draft::Edit { target }) => Some(target.item()),
        _ => None,
    }
}

#[test]
fn composer_focus_needs_an_open_room_and_loads_members_once_per_room() {
    let mut app = app();
    keys(&mut app, "<C-w>l");
    assert_eq!(keys(&mut app, "i"), vec![]);
    assert_eq!(status(&app), Some("No room is open"));
    assert_eq!(app.focus(), Focus::Timeline);

    keys(&mut app, "<C-w>h<CR>");
    assert_eq!(keys(&mut app, "i"), vec![focus(C), load_members(C)]);
    assert_eq!(app.focus(), Focus::Composer);
    normal(&mut app, "<Esc>");
    assert_eq!(app.focus(), Focus::Timeline);
    assert_eq!(keys(&mut app, "a"), vec![focus(C)]);
    normal(&mut app, "<Esc>");
    keys(&mut app, "<C-w>hj<CR>");
    assert_eq!(keys(&mut app, "i"), vec![focus(B), load_members(B)]);
}

#[test]
fn keys_resolved_after_composer_focus_use_insert_mode() {
    // `:` ends `]`, which focuses the composer in insert mode, so `:` goes
    // to the editor instead of opening the command line, as in nvim.
    let focus_insert = Binding::single(Action::ComposerFocus(FocusMode::Insert));
    let mut app = app_with(vec![
        (TableId::Timeline, seq("]"), focus_insert),
        (TableId::Normal, seq("]gq"), Binding::single(Action::Nop)),
    ]);
    keys(&mut app, "<CR>");
    assert_eq!(
        no_timers(keys(&mut app, "]:")),
        vec![
            focus(C),
            load_members(C),
            ui(UiEffect::ReplayKeys(seq(":").keys().to_vec())),
        ]
    );
    assert!(app.command_line().is_none());
}

#[test]
fn composer_keys_ask_the_tui_for_the_text_a_newline_and_the_external_editor() {
    let mut app = opened();
    keys(&mut app, "i");
    assert_eq!(
        insert(&mut app, "<CR>"),
        vec![ui(UiEffect::RequestSubmit(main_of(C)))]
    );
    assert_eq!(
        insert(&mut app, "<C-j>"),
        vec![ui(UiEffect::InsertNewline(main_of(C)))]
    );
    assert_eq!(
        normal(&mut app, "<C-x><C-e>"),
        vec![ui(UiEffect::ExternalEditor(main_of(C)))]
    );
}

#[test]
fn keys_resolved_after_a_timeout_use_the_editor_mode_of_the_last_key() {
    // The timeout runs `<C-x>`, and `<C-y>` is resolved afresh in insert
    // mode, where it is bound, though `Msg::Timeout` reports no mode.
    let mut app = app_with(vec![
        (
            TableId::ComposerInsert,
            seq("<C-x>"),
            Binding::single(Action::ComposerNewline),
        ),
        (
            TableId::ComposerInsert,
            seq("<C-x><C-y><C-z>"),
            Binding::single(Action::Nop),
        ),
        (
            TableId::ComposerInsert,
            seq("<C-y>"),
            Binding::single(Action::ComposerExternalEditor),
        ),
    ]);
    keys(&mut app, "<CR>i");
    let timer = last_timer(&keys_in(&mut app, "<C-x><C-y>", Some(EditorMode::Insert)));
    assert_eq!(
        app.update(Msg::Timeout { id: timer }),
        vec![
            ui(UiEffect::InsertNewline(main_of(C))),
            ui(UiEffect::ExternalEditor(main_of(C))),
        ]
    );
}

#[test]
fn drafts_are_kept_per_room_across_room_switches() {
    let mut app = opened();
    reset(&mut app, vec![msg(1)]);
    assert_eq!(app.draft(&main_of(C)), None);
    assert_eq!(keys(&mut app, "r"), vec![focus(C), load_members(C)]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::Reply { to: event(1) }));
    edited(&mut app, C, false);

    normal(&mut app, "<Esc>");
    keys(&mut app, "<C-w>hj<CR>");
    assert_eq!(app.open_room(), Some(&id(B)));
    assert_eq!(app.draft(&main_of(B)), None);
    app.update(Msg::Timeline {
        timeline: main_of(B),
        diffs: vec![TimelineDiff::Reset(vec![own(remote(5), "mine")])],
    });
    // The composer of `!b` is empty although the one of `!c` is not.
    keys(&mut app, "e");
    assert_eq!(edit_target(&app, B), Some(&remote(5)));

    normal(&mut app, "<Esc>");
    keys(&mut app, "<C-w>hk<CR>");
    assert_eq!(app.open_room(), Some(&id(C)));
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::Reply { to: event(1) }));
    assert_eq!(edit_target(&app, B), Some(&remote(5)));
}

#[test]
fn composer_cancel_keeps_a_reply_text_and_clears_an_edit() {
    let mut app = opened();
    reset(&mut app, vec![own(remote(1), "mine"), msg(2)]);
    keys(&mut app, "r");
    edited(&mut app, C, false);
    assert_eq!(normal(&mut app, "<C-c>"), vec![]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));

    edited(&mut app, C, true);
    normal(&mut app, "<Esc>");
    keys(&mut app, "ke");
    assert_eq!(edit_target(&app, C), Some(&remote(1)));
    assert_eq!(normal(&mut app, "<C-c>"), vec![clear(C)]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));
    // Cleared: another edit can start.
    normal(&mut app, "<Esc>");
    keys(&mut app, "e");
    assert_eq!(edit_target(&app, C), Some(&remote(1)));
}

#[test]
fn ctrl_c_goes_to_normal_mode_in_insert_mode_and_ends_a_reply_in_normal_mode() {
    // vim's insert-mode `<C-c>`, also with the mention menu open, whose
    // table falls back to `composer.insert`: the menu closes, and the reply
    // stays.
    let mut app = opened();
    reset(&mut app, vec![msg(1)]);
    keys(&mut app, "r");
    edited(&mut app, C, false);
    members(&mut app, vec![member(ALICE, "Alice")]);
    query(&mut app, Some("al"));
    assert!(menu(&app).is_some());
    assert_eq!(
        insert(&mut app, "<C-c>"),
        vec![ui(UiEffect::ComposerNormal(main_of(C)))]
    );
    assert_eq!(menu(&app), None);
    assert_eq!(app.focus(), Focus::Composer);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::Reply { to: event(1) }));
    assert_eq!(normal(&mut app, "<C-c>"), vec![]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));
}

#[test]
fn send_and_normal_run_in_order_and_keys_resolved_after_them_use_normal_mode() {
    // `<Esc>` ends the `<CR>` waiting for `<CR><C-x>` and is resolved
    // afresh: in `composer.normal` it is `composer.leave`, where insert
    // mode would pass it to the editor.
    let send_normal = Binding::new(vec![Action::ComposerSend, Action::ComposerNormal]).unwrap();
    let mut app = app_with(vec![
        (TableId::ComposerInsert, seq("<CR>"), send_normal),
        (
            TableId::ComposerInsert,
            seq("<CR><C-x>"),
            Binding::single(Action::Nop),
        ),
    ]);
    keys(&mut app, "<CR>i");
    assert_eq!(
        insert(&mut app, "<CR><Esc>"),
        vec![
            ui(UiEffect::RequestSubmit(main_of(C))),
            ui(UiEffect::ComposerNormal(main_of(C))),
        ]
    );
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn composer_normal_bound_in_the_timeline_acts_on_the_shown_composer_and_keeps_the_focus() {
    // Wherever the focus is, as `composer.send` does. The editor mode keys
    // are resolved in stays: it is the composer's only while it has focus.
    let to_normal = Binding::single(Action::ComposerNormal);
    let mut app = app_with(vec![(TableId::Timeline, seq("<C-x>"), to_normal)]);
    keys(&mut app, "<CR>");
    reset(&mut app, vec![msg(1)]);
    assert_eq!(
        keys(&mut app, "<C-x>"),
        vec![ui(UiEffect::ComposerNormal(main_of(C)))]
    );
    assert_eq!((app.focus(), app.editor), (Focus::Timeline, None));

    // In a shown thread, the thread's composer.
    keys(&mut app, "t");
    let thread = TimelineId {
        room: id(C),
        thread: Some(event(1)),
    };
    assert_eq!(app.shown_timeline(), Some(thread.clone()));
    assert_eq!(
        keys(&mut app, "<C-x>"),
        vec![ui(UiEffect::ComposerNormal(thread))]
    );
    assert_eq!((app.focus(), app.editor), (Focus::Timeline, None));
}

#[test]
fn ctrl_w_and_alt_backspace_delete_a_word_in_insert_mode_and_ctrl_w_stays_the_window_prefix() {
    let delete = || vec![ui(UiEffect::DeleteWordBefore(main_of(C)))];
    let mut app = opened();
    keys(&mut app, "i");
    for key in ["<C-w>", "<A-BS>"] {
        assert_eq!(insert(&mut app, key), delete(), "{key}");
    }
    // The mention menu's table falls back to `composer.insert`.
    edited(&mut app, C, false);
    members(&mut app, vec![member(ALICE, "Alice")]);
    query(&mut app, Some("al"));
    assert!(menu(&app).is_some());
    assert_eq!(insert(&mut app, "<C-w>"), delete());
    // In normal mode `<C-w>` is the window prefix.
    assert_eq!(normal(&mut app, "<C-w>h"), vec![]);
    assert_eq!(app.focus(), Focus::RoomList);
}

#[test]
fn composer_clear_empties_the_shown_composer_and_keeps_the_reply_and_its_mentions() {
    let clear_text = Binding::single(Action::ComposerClear);
    let mut app = app_with(vec![(TableId::ComposerInsert, seq("<C-g>"), clear_text)]);
    keys(&mut app, "<CR>");
    reset(&mut app, vec![msg(1)]);
    keys(&mut app, "r");
    members(&mut app, vec![member(ALICE, "Alice")]);
    query(&mut app, Some("al"));
    insert(&mut app, "<CR>");
    // The mention menu's table falls back to `composer.insert`, and the
    // menu closes.
    query(&mut app, Some("al"));
    assert!(menu(&app).is_some());
    assert_eq!(
        insert(&mut app, "<C-g>"),
        vec![ui(UiEffect::EmptyComposer(main_of(C)))]
    );
    assert_eq!(menu(&app), None);
    assert_eq!(app.focus(), Focus::Composer);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::Reply { to: event(1) }));
    // `u` brought the text back: it goes as the reply, with its mention.
    let mentions = [MentionRef {
        user: UserId::new(ALICE),
        text: "@Alice".into(),
    }];
    assert_eq!(
        submit(&mut app, "@Alice hi"),
        vec![
            send(
                0,
                render_mentions("@Alice hi", &mentions),
                OutgoingKind::Reply { to: event(1) }
            ),
            clear(C),
        ]
    );
}

#[test]
fn keys_resolved_after_composer_clear_in_visual_mode_use_normal_mode() {
    // The tui drops the selection. `<Esc>` ends the `<C-g>` waiting for
    // `<C-g><C-x>` and is resolved afresh: in `composer.normal` it is
    // `composer.leave`, where `composer.visual` would pass it to the editor.
    let clear_text = Binding::single(Action::ComposerClear);
    let mut app = app_with(vec![
        (TableId::ComposerVisual, seq("<C-g>"), clear_text),
        (
            TableId::ComposerVisual,
            seq("<C-g><C-x>"),
            Binding::single(Action::Nop),
        ),
    ]);
    keys(&mut app, "<CR>i");
    assert_eq!(
        no_timers(keys_in(&mut app, "<C-g><Esc>", Some(EditorMode::Visual))),
        vec![ui(UiEffect::EmptyComposer(main_of(C)))]
    );
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn a_refused_register_is_an_error_in_the_status_line() {
    let mut app = opened();
    let error = "Only the black hole register \"_ is supported";
    assert_eq!(app.update(Msg::EditorRefused(error.into())), vec![]);
    assert_eq!(status(&app), Some(error));
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
}

#[test]
fn composer_leave_ends_a_reply_without_text() {
    let mut app = opened();
    reset(&mut app, vec![msg(1)]);
    keys(&mut app, "r");
    assert_eq!(normal(&mut app, "<Esc>"), vec![]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn composer_leave_ends_an_edit_without_text_and_leaves_the_message_alone() {
    let mut app = opened();
    reset(&mut app, vec![own(remote(1), "mine")]);
    keys(&mut app, "e");
    // The user deleted the loaded text.
    edited(&mut app, C, true);
    // Nothing is sent: the message keeps its text.
    assert_eq!(normal(&mut app, "<Esc>"), vec![clear(C)]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn composer_leave_keeps_any_text_with_its_draft() {
    let mut app = opened();
    reset(&mut app, vec![own(remote(1), "mine"), msg(2)]);
    // A reply with text, also whitespace alone, which the tui reports as
    // text.
    keys(&mut app, "r");
    edited(&mut app, C, false);
    assert_eq!(normal(&mut app, "<Esc>"), vec![]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::Reply { to: event(2) }));
    assert_eq!(app.focus(), Focus::Timeline);

    // A new draft with text: the text stays, so an edit is refused.
    keys(&mut app, "i");
    normal(&mut app, "<C-c>");
    assert_eq!(normal(&mut app, "<Esc>"), vec![]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));
    assert_eq!(app.focus(), Focus::Timeline);
    keys(&mut app, "ke");
    assert_eq!(status(&app), Some("Clear the composer first"));

    // An edit with its loaded text.
    edited(&mut app, C, true);
    keys(&mut app, "e");
    assert_eq!(normal(&mut app, "<Esc>"), vec![]);
    assert_eq!(edit_target(&app, C), Some(&remote(1)));
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn message_edit_loads_the_plain_body_of_an_own_text_message_with_its_mentions() {
    let alice = UserId::new(ALICE);
    let italic = Style {
        italic: true,
        ..Style::default()
    };
    // `@Alice *hi*` as sent, its formatted body shown as a pill and an
    // italic `hi`.
    let rich = RichText {
        blocks: vec![Block::Paragraph(vec![
            Inline::Mention {
                text: "@Alice".into(),
                target: MentionTarget::User(alice.clone()),
                spoiler: false,
            },
            Inline::Text {
                text: " ".into(),
                style: Style::default(),
            },
            Inline::Text {
                text: "hi".into(),
                style: italic,
            },
        ])],
    };
    let image = MessageContent::Image {
        name: "cat.png".into(),
        size: None,
        dimensions: None,
    };
    let mut app = opened();
    reset(
        &mut app,
        vec![
            with(own(remote(1), ""), |item| {
                item.content = MessageContent::Text {
                    body: "@Alice *hi*".into(),
                    rich,
                };
            }),
            with(own(remote(2), ""), |item| item.content = image),
            msg(3),
        ],
    );
    assert_eq!(keys(&mut app, "e"), vec![]);
    assert_eq!(status(&app), Some("You can only edit your own messages"));
    keys(&mut app, "k");
    assert_eq!(keys(&mut app, "e"), vec![]);
    assert_eq!(status(&app), Some("Only text messages can be edited"));
    keys(&mut app, "k");
    edited(&mut app, C, false);
    assert_eq!(keys(&mut app, "e"), vec![]);
    assert_eq!(status(&app), Some("Clear the composer first"));
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));

    edited(&mut app, C, true);
    assert_eq!(
        keys(&mut app, "e"),
        vec![
            focus(C),
            load_members(C),
            ui(UiEffect::LoadComposer {
                timeline: main_of(C),
                text: "@Alice *hi*".into(),
            }),
        ]
    );
    assert_eq!(edit_target(&app, C), Some(&remote(1)));
    assert_eq!(app.focus(), Focus::Composer);
    // The rebuilt mention is linked when the edit is sent.
    let mentions = [MentionRef {
        user: alice,
        text: "@Alice".into(),
    }];
    assert_eq!(
        submit(&mut app, "@Alice *hi* again"),
        vec![
            send(
                0,
                render_mentions("@Alice *hi* again", &mentions),
                OutgoingKind::Edit { target: remote(1) }
            ),
            clear(C),
        ]
    );
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));
}

#[test]
fn submit_sends_by_the_draft_and_clears_the_composer() {
    let mut app = opened();
    reset(&mut app, vec![msg(1)]);
    keys(&mut app, "i");
    assert_eq!(submit(&mut app, " \n "), vec![]);
    assert_eq!(
        submit(&mut app, "hello"),
        vec![send(0, plain("hello"), OutgoingKind::New), clear(C)]
    );

    normal(&mut app, "<Esc>");
    keys(&mut app, "r");
    assert_eq!(
        submit(&mut app, "re"),
        vec![
            send(1, plain("re"), OutgoingKind::Reply { to: event(1) }),
            clear(C),
        ]
    );
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));
}

/// The matrix adapter could not queue the app's `n`-th send.
fn failed(app: &mut App, n: u64) -> Vec<Effect> {
    app.update(Msg::SendFailed {
        id: SendId(n),
        error: "Sending the message failed: boom".into(),
    })
}

#[test]
fn a_send_that_cannot_be_queued_puts_its_draft_back() {
    let mut app = opened();
    reset(&mut app, vec![msg(1)]);
    keys(&mut app, "r");
    members(&mut app, vec![member(ALICE, "Alice")]);
    query(&mut app, Some("al"));
    insert(&mut app, "<CR>");
    submit(&mut app, "@Alice hi");
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));

    assert_eq!(
        failed(&mut app, 0),
        vec![ui(UiEffect::LoadComposer {
            timeline: main_of(C),
            text: "@Alice hi".into(),
        })]
    );
    assert_eq!(status(&app), Some("Sending the message failed: boom"));
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::Reply { to: event(1) }));
    // Sent again as a reply, with its mention.
    let mentions = [MentionRef {
        user: UserId::new(ALICE),
        text: "@Alice".into(),
    }];
    assert_eq!(
        submit(&mut app, "@Alice hi"),
        vec![
            send(
                1,
                render_mentions("@Alice hi", &mentions),
                OutgoingKind::Reply { to: event(1) }
            ),
            clear(C),
        ]
    );

    // A queued send has nothing left to put back.
    app.update(Msg::SendQueued { id: SendId(1) });
    assert_eq!(failed(&mut app, 1), vec![]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));
}

#[test]
fn a_failed_send_does_not_replace_what_the_user_started_since() {
    let mut app = opened();
    reset(&mut app, vec![msg(1), msg(2)]);
    keys(&mut app, "i");
    submit(&mut app, "first");
    // The next message is being typed.
    edited(&mut app, C, false);
    assert_eq!(failed(&mut app, 0), vec![]);
    assert_eq!(status(&app), Some("Sending the message failed: boom"));
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));

    // A reply started in the empty composer stays a reply to its message.
    edited(&mut app, C, true);
    submit(&mut app, "second");
    normal(&mut app, "<Esc>");
    keys(&mut app, "r");
    assert_eq!(failed(&mut app, 1), vec![]);
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::Reply { to: event(2) }));
}

#[test]
fn a_failed_send_is_not_put_back_after_switching_rooms() {
    let mut app = opened();
    keys(&mut app, "i");
    submit(&mut app, "hello");
    normal(&mut app, "<Esc>");
    keys(&mut app, "<C-w>hj<CR>");
    assert_eq!(app.open_room(), Some(&id(B)));
    assert_eq!(failed(&mut app, 0), vec![]);
    assert_eq!(status(&app), Some("Sending the message failed: boom"));
    assert_eq!(app.draft(&main_of(B)), None);

    // Nor when its room opens again.
    let back = keys(&mut app, "<C-w>hk<CR>");
    assert_eq!(app.open_room(), Some(&id(C)));
    assert!(
        !back
            .iter()
            .any(|effect| matches!(effect, Effect::Ui(UiEffect::LoadComposer { .. })))
    );
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::New));
}

/// `!c` open with the user's `@Alice typo`, its mention a pill, being edited.
fn editing() -> App {
    let rich = RichText {
        blocks: vec![Block::Paragraph(vec![
            Inline::Mention {
                text: "@Alice".into(),
                target: MentionTarget::User(UserId::new(ALICE)),
                spoiler: false,
            },
            Inline::Text {
                text: " typo".into(),
                style: Style::default(),
            },
        ])],
    };
    let mut app = opened();
    let message = with(own(remote(1), "@Alice typo"), |item| {
        item.content = MessageContent::Text {
            body: "@Alice typo".into(),
            rich,
        };
    });
    reset(&mut app, vec![message]);
    keys(&mut app, "e");
    app
}

/// The homeserver refused the app's `n`-th send, an edit of a sent message,
/// after the send queue had it.
fn refused(app: &mut App, n: u64) -> Vec<Effect> {
    app.update(Msg::SendFailed {
        id: SendId(n),
        error: "Editing the message failed: the homeserver answered 403".into(),
    })
}

#[test]
fn an_edit_refused_after_it_was_queued_comes_back_as_an_edit_of_its_message() {
    let mut app = editing();
    submit(&mut app, "@Alice fixed");
    // A message sent meanwhile is queued and leaves the composer empty.
    submit(&mut app, "next");
    app.update(Msg::SendQueued { id: SendId(1) });

    assert_eq!(
        refused(&mut app, 0),
        vec![ui(UiEffect::LoadComposer {
            timeline: main_of(C),
            text: "@Alice fixed".into(),
        })]
    );
    assert_eq!(
        status(&app),
        Some("Editing the message failed: the homeserver answered 403")
    );
    assert_eq!(edit_target(&app, C), Some(&remote(1)));
    // Sent again as an edit of the same message, with its mention.
    let mentions = [MentionRef {
        user: UserId::new(ALICE),
        text: "@Alice".into(),
    }];
    assert_eq!(
        submit(&mut app, "@Alice fixed"),
        vec![
            send(
                2,
                render_mentions("@Alice fixed", &mentions),
                OutgoingKind::Edit { target: remote(1) }
            ),
            clear(C),
        ]
    );
}

#[test]
fn message_reply_refuses_a_local_echo_and_an_edit_draft() {
    let mut app = opened();
    reset(
        &mut app,
        vec![own(remote(1), "mine"), own(local("t1"), "pending")],
    );
    // The echo's remote event replaces it, so a reply to it could never be
    // sent.
    assert_eq!(keys(&mut app, "r"), vec![]);
    assert_eq!(status(&app), Some("Wait until the message is sent"));
    assert_eq!(app.draft(&main_of(C)), None);
    assert_eq!(app.focus(), Focus::Timeline);

    // The composer holds the edit's text, which must not become a reply.
    keys(&mut app, "ke");
    normal(&mut app, "<Esc>");
    assert_eq!(keys(&mut app, "r"), vec![]);
    assert_eq!(status(&app), Some("Cancel the edit first"));
    assert_eq!(edit_target(&app, C), Some(&remote(1)));
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn the_mention_menu_ranks_members_and_records_accepted_mentions() {
    let alfred = "@alfred:example.org";
    let mut app = opened();
    // Alice sent a loaded message, and so did the user, who is not ranked
    // as a recent sender.
    reset(&mut app, vec![msg(1), own(remote(2), "mine")]);
    keys(&mut app, "i");
    query(&mut app, Some("al"));
    assert_eq!(menu(&app), None);
    members(
        &mut app,
        vec![
            member(alfred, "Alfred"),
            member(ME, "Alma"),
            member(ALICE, "Alice"),
            member("@bob:example.org", "Bob"),
        ],
    );
    // The open query is ranked again when the members arrive.
    assert_eq!(menu(&app), Some((vec![ALICE, ME, alfred], 0)));

    insert(&mut app, "<C-n><C-n>");
    assert_eq!(menu(&app), Some((vec![ALICE, ME, alfred], 2)));
    insert(&mut app, "<C-n>");
    assert_eq!(menu(&app).unwrap().1, 0);
    insert(&mut app, "<C-p>");
    assert_eq!(menu(&app).unwrap().1, 2);
    assert_eq!(
        insert(&mut app, "<CR>"),
        vec![ui(UiEffect::ReplaceMentionToken {
            timeline: main_of(C),
            text: "@Alfred".into(),
        })]
    );
    assert_eq!(menu(&app), None);
    let mentions = [MentionRef {
        user: UserId::new(alfred),
        text: "@Alfred".into(),
    }];
    assert_eq!(
        submit(&mut app, "@Alfred hi"),
        vec![
            send(
                0,
                render_mentions("@Alfred hi", &mentions),
                OutgoingKind::New
            ),
            clear(C),
        ]
    );

    // `completion.cancel` closes the menu, and `<CR>` sends again.
    query(&mut app, Some("b"));
    assert_eq!(menu(&app), Some((vec!["@bob:example.org"], 0)));
    assert_eq!(insert(&mut app, "<C-e>"), vec![]);
    assert_eq!(menu(&app), None);
    assert_eq!(
        insert(&mut app, "<CR>"),
        vec![ui(UiEffect::RequestSubmit(main_of(C)))]
    );
    // No token closes it; a query without candidates opens none.
    query(&mut app, Some("b"));
    query(&mut app, None);
    assert_eq!(menu(&app), None);
    query(&mut app, Some("zzz"));
    assert_eq!(menu(&app), None);
    // A key outside insert mode closes it, so `<CR>` sends afterwards.
    query(&mut app, Some("b"));
    normal(&mut app, "x");
    assert_eq!(menu(&app), None);
    assert_eq!(
        insert(&mut app, "<CR>"),
        vec![ui(UiEffect::RequestSubmit(main_of(C)))]
    );
    // Only the focused composer has a menu.
    normal(&mut app, "<Esc>");
    query(&mut app, Some("b"));
    assert_eq!(menu(&app), None);
    keys(&mut app, "i");
    assert_eq!(menu(&app), None);
}

#[test]
fn the_mention_menu_keeps_the_best_candidates() {
    let mut app = opened();
    keys(&mut app, "i");
    let all: Vec<Member> = (0..60)
        .map(|n| member(&format!("@user{n:02}:example.org"), &format!("User {n:02}")))
        .collect();
    members(&mut app, all);
    query(&mut app, Some(""));
    let (candidates, _) = menu(&app).unwrap();
    assert_eq!(candidates.len(), 50);
    assert_eq!(candidates[0], "@user00:example.org");
    assert_eq!(candidates[49], "@user49:example.org");
}

#[test]
fn completion_accept_needs_the_menu_of_the_focused_composer() {
    let away = Binding::new(vec![Action::FocusTimeline, Action::CompletionAccept]).unwrap();
    let mut app = app_with(vec![(TableId::ComposerCompletion, seq("<C-t>"), away)]);
    keys(&mut app, "<CR>i");
    members(&mut app, vec![member(ALICE, "Alice")]);
    query(&mut app, Some("al"));
    assert!(menu(&app).is_some());
    // The token is in an editor without focus, so it is not replaced.
    assert_eq!(insert(&mut app, "<C-t>"), vec![]);
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn message_delete_asks_for_confirmation() {
    let mut app = opened();
    reset(&mut app, vec![own(remote(1), "mine"), msg(2)]);
    assert_eq!(no_timers(keys(&mut app, "dd")), vec![]);
    assert_eq!(status(&app), Some("You can only delete your own messages"));
    assert_eq!(app.confirmation(), None);

    keys(&mut app, "k");
    assert_eq!(no_timers(keys(&mut app, "dd")), vec![]);
    assert_eq!(
        app.confirmation().as_deref(),
        Some("Delete this message? (y/n)")
    );
    assert_eq!(app.mode_label(None), "CONFIRM");
    assert_eq!(
        keys(&mut app, "y"),
        vec![matrix(MatrixEffect::Redact {
            room: id(C),
            item: remote(1),
        })]
    );
    // Any other key cancels, and goes nowhere else.
    keys(&mut app, "dd");
    assert_eq!(keys(&mut app, "j"), vec![]);
    assert_eq!(app.confirmation(), None);
    assert_eq!(app.timeline_cursor(), Some(0));
    assert_eq!(app.mode_label(None), "NORMAL");
}

#[test]
fn the_confirm_question_is_not_a_status_message() {
    let mut app = opened();
    reset(&mut app, vec![own(remote(1), "mine")]);
    keys(&mut app, "dd");
    assert_eq!(
        app.confirmation().as_deref(),
        Some("Delete this message? (y/n)")
    );
    keys(&mut app, "n");
    assert_eq!(messages(&mut app), [MESSAGES_FIRST_LINE]);
}

#[test]
fn keys_left_after_a_binding_which_asks_for_confirmation_are_dropped() {
    // `g` is also a prefix of `gg` and `gyq`, so a key that matches nothing
    // or the timeout ends it. `message.delete` runs, and the keys after `g`
    // are dropped instead of answering, so keys typed ahead never confirm a
    // redaction.
    let mut app = app_with(vec![
        (
            TableId::Timeline,
            seq("g"),
            Binding::single(Action::MessageDelete),
        ),
        (TableId::Timeline, seq("gyq"), Binding::single(Action::Nop)),
    ]);
    keys(&mut app, "<CR>");
    reset(&mut app, vec![own(remote(1), "one"), own(remote(2), "two")]);
    let question = Some("Delete this message? (y/n)");
    assert_eq!(no_timers(keys(&mut app, "gyk")), vec![]);
    assert_eq!(app.confirmation().as_deref(), question);
    assert_eq!(app.timeline_cursor(), Some(1));
    // A key typed after the question answers it.
    assert_eq!(keys(&mut app, "n"), vec![]);
    assert_eq!(app.confirmation(), None);

    let timer = last_timer(&keys(&mut app, "gy"));
    assert_eq!(app.update(Msg::Timeout { id: timer }), vec![]);
    assert_eq!(app.confirmation().as_deref(), question);
    assert_eq!(
        keys(&mut app, "y"),
        vec![matrix(MatrixEffect::Redact {
            room: id(C),
            item: remote(2),
        })]
    );
}

#[test]
fn a_paste_never_answers_a_question() {
    let mut app = opened();
    reset(&mut app, vec![own(remote(1), "one")]);
    no_timers(keys(&mut app, "dd"));
    assert_eq!(
        app.confirmation().as_deref(),
        Some("Delete this message? (y/n)")
    );
    assert_eq!(paste(&mut app, "y", None), vec![]);
    assert_eq!(app.confirmation(), None);
    assert_eq!(no_timers(keys(&mut app, "y")), vec![]);
}

#[test]
fn a_paste_never_answers_a_question_that_the_keys_it_ends_ask() {
    let mut app = app_with(vec![
        (
            TableId::Timeline,
            seq("g"),
            Binding::single(Action::MessageDelete),
        ),
        (TableId::Timeline, seq("gyq"), Binding::single(Action::Nop)),
    ]);
    keys(&mut app, "<CR>");
    reset(&mut app, vec![own(remote(1), "one"), own(remote(2), "two")]);
    let question = Some("Delete this message? (y/n)");
    keys(&mut app, "g");
    let effects = paste(&mut app, "y\ny", None);
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Matrix(_))),
        "{effects:?}"
    );
    assert_eq!(app.confirmation().as_deref(), question);
    keys(&mut app, "n");
    // `gy` pending: `g` asks, and the `y` fed again is dropped.
    keys(&mut app, "gy");
    let effects = paste(&mut app, "y", None);
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Matrix(_))),
        "{effects:?}"
    );
    assert_eq!(app.confirmation().as_deref(), question);
}

#[test]
fn a_question_asked_with_the_recovery_key_prompt_is_shown_and_answered_first() {
    use Action::{AccountRecover, MessageDelete};
    for actions in [
        vec![MessageDelete, AccountRecover],
        vec![AccountRecover, MessageDelete],
    ] {
        let both = Binding::new(actions).unwrap();
        let mut app = app_with(vec![(TableId::Timeline, seq("D"), both)]);
        keys(&mut app, "<CR>");
        reset(&mut app, vec![own(remote(1), "mine")]);
        assert_eq!(no_timers(keys(&mut app, "D")), vec![]);
        // The bottom line shows the question, which the next key answers.
        assert_eq!(app.mode_label(None), "CONFIRM");
        assert_eq!(
            app.confirmation().as_deref(),
            Some("Delete this message? (y/n)")
        );
        assert_eq!(app.secret_prompt(), None);
        assert_eq!(
            keys(&mut app, "y"),
            vec![matrix(MatrixEffect::Redact {
                room: id(C),
                item: remote(1),
            })]
        );
        // The answer is not part of the key.
        assert_eq!(app.mode_label(None), "SECRET");
        assert_eq!(secret_len(&app), Some(0));
    }
}

#[test]
fn deleted_messages_cannot_be_deleted_or_reacted_to() {
    let mut app = opened();
    reset(
        &mut app,
        vec![with(own(remote(1), ""), |item| {
            item.content = MessageContent::Redacted;
        })],
    );
    assert_eq!(no_timers(keys(&mut app, "dd")), vec![]);
    assert_eq!(status(&app), Some("The message is deleted"));
    assert_eq!(app.confirmation(), None);
    keys(&mut app, "+");
    assert_eq!(status(&app), Some("The message is deleted"));
    assert!(app.picker().is_none());
}

#[test]
fn opening_another_room_or_losing_the_open_room_closes_what_belongs_to_it() {
    let next = Binding::single(Action::RoomOpen(RoomTarget::Pick {
        filter: RoomFilter::All,
        pick: Pick::Next,
    }));
    let mut app = app_with(vec![
        (TableId::ComposerInsert, seq("<C-o>"), next.clone()),
        (TableId::Picker, seq("<C-o>"), next),
    ]);
    let timeline = |app: &mut App, room: &str, entry: TimelineEntry| {
        app.update(Msg::Timeline {
            timeline: main_of(room),
            diffs: vec![TimelineDiff::Reset(vec![entry])],
        });
    };
    // `!c`: the composer has focus.
    keys(&mut app, "<CR>i");
    insert(&mut app, "<C-o>");
    assert_eq!(app.open_room(), Some(&id(B)));
    assert_eq!(app.focus(), Focus::Timeline);
    // `!b`: the emoji picker is open.
    timeline(&mut app, B, msg(1));
    keys(&mut app, "+");
    keys(&mut app, "<C-o>");
    assert_eq!(app.open_room(), Some(&id(A)));
    assert!(app.picker().is_none());
    assert_eq!(app.mode_label(None), "NORMAL");
    // `!a`: a question is asked, and the room goes away.
    timeline(&mut app, A, own(remote(2), "mine"));
    keys(&mut app, "dd");
    assert!(app.confirmation().is_some());
    app.update(Msg::RoomRemoved(id(A)));
    assert_eq!(app.open_room(), None);
    assert_eq!(app.confirmation(), None);
}

fn reacted(key: &str, mine: bool) -> ReactionGroup {
    ReactionGroup {
        key: key.into(),
        count: 1,
        mine,
    }
}
/// The keys of the open picker's first `n` items.
fn first_keys(app: &App, n: usize) -> Vec<PickerKey> {
    let items = &app.picker().unwrap().items[..n];
    items.iter().map(|item| item.key.clone()).collect()
}
fn reaction_keys<const N: usize>(keys: [&str; N]) -> Vec<PickerKey> {
    keys.map(|key| PickerKey::Reaction(key.into())).to_vec()
}

#[test]
fn the_emoji_picker_lists_the_shown_reactions_then_common_emoji_and_toggles_one() {
    let mut app = opened();
    reset(
        &mut app,
        vec![with(msg(1), |item| {
            // ❤ without its variation selector, as some clients send it,
            // and a key that is no emoji.
            item.reactions = vec![
                reacted("👍", false),
                reacted("🎉", true),
                reacted("lol", false),
                reacted("\u{2764}", true),
            ];
        })],
    );
    keys(&mut app, "+");
    let picker = app.picker().unwrap();
    assert_eq!(
        picker.kind,
        PickerKind::Emoji {
            room: id(C),
            item: remote(1),
        }
    );
    assert_eq!(app.mode_label(None), "PICKER");
    // The reactions as the timeline shows them, the user's own among them,
    // each with its own key; then the other common reactions, then every
    // other emoji in `emojis` order.
    let first_other = emoji::all().next().unwrap().as_str();
    assert_eq!(
        first_keys(&app, 10),
        reaction_keys([
            "👍",
            "🎉",
            "lol",
            "\u{2764}",
            "😂",
            "😮",
            "😢",
            "👀",
            "🙏",
            first_other
        ])
    );
    let picker = app.picker().unwrap();
    assert_eq!(picker.items[2].label, "lol");
    // Labelled with the key as sent, which lacks the variation selector.
    assert_eq!(picker.items[3].label, ":heart: \u{2764}");
    // Each emoji once: the shown ones are not listed again.
    let labels: HashSet<&str> = picker.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels.len(), picker.items.len());
    assert_eq!(picker.items.len(), emoji::all().count() + 1);

    // `<CR>` toggles the first shown reaction, `<C-n><CR>` the second.
    assert_eq!(keys(&mut app, "<CR>"), vec![toggle(1, "👍")]);
    keys(&mut app, "+");
    assert_eq!(keys(&mut app, "<C-n><CR>"), vec![toggle(1, "🎉")]);
    keys(&mut app, "+");
    assert_eq!(keys(&mut app, "<C-n><C-n><CR>"), vec![toggle(1, "lol")]);

    keys(&mut app, "+");
    // Typed keys go to the query, not to the timeline bindings.
    keys(&mut app, "jk");
    assert_eq!(app.picker().unwrap().query.text(), "jk");
    assert_eq!(app.timeline_cursor(), Some(0));
    keys(&mut app, "<C-u>heart");
    let picker = app.picker().unwrap();
    // The user's own reaction key, so accepting it removes the reaction.
    assert_eq!(
        picker.items[0],
        PickerItem {
            label: ":heart: ❤️".into(),
            badge: None,
            key: PickerKey::Reaction("\u{2764}".into()),
        }
    );
    keys(&mut app, "<C-n><C-n><C-p>");
    assert_eq!(app.picker().unwrap().selected, 1);
    keys(&mut app, "<C-u>thumbsup");
    assert_eq!(app.picker().unwrap().selected, 0);
    assert_eq!(
        keys(&mut app, "<CR>"),
        vec![matrix(MatrixEffect::ToggleReaction {
            room: id(C),
            item: remote(1),
            key: "👍".into(),
        })]
    );
    assert!(app.picker().is_none());
    assert_eq!(app.mode_label(None), "NORMAL");

    keys(&mut app, "+heart");
    assert_eq!(keys(&mut app, "<Esc>"), vec![]);
    assert!(app.picker().is_none());
    assert_eq!(app.timeline_cursor(), Some(0));
}

#[test]
fn the_emoji_picker_toggles_each_shown_reaction_with_its_own_key() {
    let mut app = opened();
    let open = |app: &mut App, reactions: Vec<ReactionGroup>| {
        reset(app, vec![with(msg(1), |item| item.reactions = reactions)]);
        keys(app, "+");
    };
    let labels = |app: &App, n: usize| -> Vec<String> {
        let items = &app.picker().unwrap().items[..n];
        items.iter().map(|item| item.label.clone()).collect()
    };
    // Another user's ❤ without its variation selector: choosing it joins
    // their reaction, and ❤️ is not listed again among the common emoji.
    open(&mut app, vec![reacted("\u{2764}", false)]);
    assert_eq!(labels(&app, 1), [":heart: \u{2764}"]);
    assert_eq!(first_keys(&app, 3), reaction_keys(["\u{2764}", "👍", "😂"]));
    let qualified = PickerKey::Reaction("\u{2764}\u{fe0f}".into());
    let items = &app.picker().unwrap().items;
    assert!(items.iter().all(|item| item.key != qualified));
    assert_eq!(keys(&mut app, "<CR>"), vec![toggle(1, "\u{2764}")]);

    // Another user's ❤ and the user's own ❤️: two entries in the shown
    // order, told apart by their labels, each toggling its own key.
    open(
        &mut app,
        vec![
            reacted("\u{2764}", false),
            reacted("\u{2764}\u{fe0f}", true),
        ],
    );
    assert_eq!(
        labels(&app, 2),
        [":heart: \u{2764}", ":heart: \u{2764}\u{fe0f}"]
    );
    assert_eq!(
        first_keys(&app, 3),
        reaction_keys(["\u{2764}", "\u{2764}\u{fe0f}", "👍"])
    );
    assert_eq!(keys(&mut app, "<CR>"), vec![toggle(1, "\u{2764}")]);
    keys(&mut app, "+");
    assert_eq!(
        keys(&mut app, "<C-n><CR>"),
        vec![toggle(1, "\u{2764}\u{fe0f}")]
    );
}

#[test]
fn the_emoji_picker_without_reactions_lists_the_common_emoji_first() {
    let mut app = opened();
    reset(&mut app, vec![msg(1)]);
    keys(&mut app, "+");
    let first_other = emoji::all().next().unwrap().as_str();
    assert_eq!(
        first_keys(&app, 9),
        reaction_keys(["👍", "❤️", "😂", "😮", "😢", "🎉", "👀", "🙏", first_other])
    );
    assert_eq!(app.picker().unwrap().items.len(), emoji::all().count());
    assert_eq!(keys(&mut app, "<CR>"), vec![toggle(1, "👍")]);
}

fn toggle(n: u32, key: &str) -> Effect {
    matrix(MatrixEffect::ToggleReaction {
        room: id(C),
        item: remote(n),
        key: key.into(),
    })
}
fn command(app: &mut App, line: &str) -> Vec<Effect> {
    no_timers(keys(app, &format!(":{line}<CR>")))
}

#[test]
fn react_with_an_emoji_toggles_it_with_the_users_own_key() {
    let mut app = opened();
    reset(&mut app, vec![msg(1)]);
    for argument in ["👍", "thumbsup", ":+1:"] {
        assert_eq!(
            command(&mut app, &format!("react {argument}")),
            vec![toggle(1, "👍")]
        );
    }
    // Without the user's own reaction (another user's does not count), the
    // emoji itself.
    let heart = |mine: bool| ReactionGroup {
        key: "\u{2764}".into(),
        count: 1,
        mine,
    };
    reset(
        &mut app,
        vec![with(msg(1), |item| item.reactions = vec![heart(false)])],
    );
    assert_eq!(
        command(&mut app, "react heart"),
        vec![toggle(1, "\u{2764}\u{fe0f}")]
    );
    // The user reacted with `❤`, without the variation selector: its key, so
    // the reaction goes instead of a second one coming.
    reset(
        &mut app,
        vec![with(msg(1), |item| item.reactions = vec![heart(true)])],
    );
    assert_eq!(
        command(&mut app, "react heart"),
        vec![toggle(1, "\u{2764}")]
    );
    // Unknown: a status, and nothing is sent.
    for argument in [":+1", "nonsense"] {
        assert_eq!(command(&mut app, &format!("react {argument}")), vec![]);
        assert_eq!(app.status().unwrap().kind, StatusKind::Error);
        let unknown = format!("unknown emoji \"{argument}\"");
        assert_eq!(status(&app), Some(unknown.as_str()));
    }
}

#[test]
fn react_acts_on_the_open_rooms_timeline_cursor_from_the_room_list_too() {
    let mut app = opened();
    reset(&mut app, vec![msg(1), msg(2)]);
    keys(&mut app, "k<C-w>h");
    assert_eq!(app.focus(), Focus::RoomList);
    assert_eq!(command(&mut app, "react 👍"), vec![toggle(1, "👍")]);
    // Without an argument, the emoji picker.
    assert_eq!(command(&mut app, "react"), vec![]);
    let picker = app.picker().unwrap();
    assert_eq!(
        picker.kind,
        PickerKind::Emoji {
            room: id(C),
            item: remote(1),
        }
    );
}

/// `g1` to `g9`, bound by default in `timeline`.
#[test]
fn react_with_an_index_toggles_the_reaction_shown_there() {
    let group = |key: &str, count: u32, mine: bool| ReactionGroup {
        key: key.into(),
        count,
        mine,
    };
    let mut app = opened();
    let shown = |thumbs_mine: bool| {
        vec![
            group("👍", 2, thumbs_mine),
            // The user's heart, without the variation selector.
            group("\u{2764}", 1, true),
            // Another user's heart, with it.
            group("\u{2764}\u{fe0f}", 1, false),
            // Not an emoji `emojis` knows.
            group("lgtm", 4, false),
        ]
    };
    reset(
        &mut app,
        vec![with(msg(1), |item| item.reactions = shown(false))],
    );
    // Another user's reaction: the user's joins it with its key.
    assert_eq!(no_timers(keys(&mut app, "g1")), vec![toggle(1, "👍")]);
    // The user's own reaction: its key, which removes it.
    assert_eq!(no_timers(keys(&mut app, "g2")), vec![toggle(1, "\u{2764}")]);
    // Another user's reaction with the emoji the user reacted with under
    // another key: its own key, so the user's joins it rather than removing
    // the second chip.
    assert_eq!(
        no_timers(keys(&mut app, "g3")),
        vec![toggle(1, "\u{2764}\u{fe0f}")]
    );
    assert_eq!(no_timers(keys(&mut app, "g4")), vec![toggle(1, "lgtm")]);
    // Past the last reaction: a status, and nothing is sent.
    assert_eq!(no_timers(keys(&mut app, "g5")), vec![]);
    assert_eq!(status(&app), Some("The message has no reaction 5"));
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);

    // The SDK shows the user's 👍 after the first `g1`: the second removes
    // it with its key.
    reset(
        &mut app,
        vec![with(msg(1), |item| item.reactions = shown(true))],
    );
    assert_eq!(no_timers(keys(&mut app, "g1")), vec![toggle(1, "👍")]);

    // The SDK shows the user's ❤️ after `g3`, next to their ❤: the second
    // `g3` removes it with its key, and `g2` still removes the ❤.
    let mut joined = shown(false);
    joined[2] = group("\u{2764}\u{fe0f}", 2, true);
    reset(&mut app, vec![with(msg(1), |item| item.reactions = joined)]);
    assert_eq!(
        no_timers(keys(&mut app, "g3")),
        vec![toggle(1, "\u{2764}\u{fe0f}")]
    );
    assert_eq!(no_timers(keys(&mut app, "g2")), vec![toggle(1, "\u{2764}")]);

    // A message without reactions has none to toggle.
    reset(&mut app, vec![msg(2)]);
    assert_eq!(no_timers(keys(&mut app, "g1")), vec![]);
    assert_eq!(status(&app), Some("The message has no reaction 1"));
}

/// `gN` toggles the key the emoji picker's N-th entry toggles, whichever
/// chip is shown at N: the user's own, another user's, one with the emoji of
/// the user's own under another key, or a key that is no emoji.
#[test]
fn react_with_an_index_toggles_the_key_of_the_emoji_pickers_entry_there() {
    let chips = vec![
        reacted("\u{2764}\u{fe0f}", true),
        reacted("👍", false),
        reacted("\u{2764}", false),
        reacted("lgtm", true),
        reacted("🎉", true),
        reacted("lol", false),
        reacted("😂", false),
        reacted("👍🏽", true),
        reacted("🙏", false),
    ];
    let mut app = opened();
    reset(
        &mut app,
        vec![with(msg(1), |item| item.reactions = chips.clone())],
    );
    for (n, chip) in (1..).zip(&chips) {
        let index = no_timers(keys(&mut app, &format!("g{n}")));
        assert_eq!(index, vec![toggle(1, &chip.key)], "g{n}");
        keys(&mut app, "+");
        let next = "<C-n>".repeat(n - 1);
        let picked = no_timers(keys(&mut app, &format!("{next}<CR>")));
        assert_eq!(picked, index, "the picker's entry {n}");
    }
}

/// The `emoji` argument names an emoji, not a chip: the user's own key with
/// that emoji, else the emoji itself, under the chips where `g3` joins ❤️.
#[test]
fn react_with_an_emoji_keeps_the_users_own_key_under_any_chips() {
    let heart = Emoji::parse("heart").unwrap();
    let bound = Binding::single(Action::MessageReact(ReactWith::Emoji(heart)));
    let mut app = app_with(vec![(TableId::Timeline, seq("<F2>"), bound)]);
    keys(&mut app, "<CR>");
    reset(
        &mut app,
        vec![with(msg(1), |item| {
            item.reactions = vec![
                reacted("👍", false),
                reacted("\u{2764}", true),
                reacted("\u{2764}\u{fe0f}", false),
            ];
        })],
    );
    assert_eq!(
        command(&mut app, "react heart"),
        vec![toggle(1, "\u{2764}")]
    );
    assert_eq!(
        no_timers(keys(&mut app, "<F2>")),
        vec![toggle(1, "\u{2764}")]
    );
}

#[test]
fn react_and_plus_say_why_they_do_nothing() {
    let react = |app: &mut App| {
        assert_eq!(command(app, "react 👍"), vec![]);
        let said = status(app).map(str::to_owned);
        assert_eq!(no_timers(keys(app, "+")), vec![]);
        assert!(app.picker().is_none());
        assert_eq!(app.status().unwrap().kind, StatusKind::Error);
        assert_eq!(status(app).map(str::to_owned), said);
        // `index` checks the message first too.
        assert_eq!(no_timers(keys(app, "g1")), vec![]);
        assert_eq!(status(app).map(str::to_owned), said);
        said
    };
    let mut app = app();
    // `+` is bound in the timeline, which has focus without an open room.
    keys(&mut app, "<C-w>l");
    assert_eq!(react(&mut app).as_deref(), Some("No room is open"));
    keys(&mut app, "<C-w>h<CR>");
    assert_eq!(react(&mut app).as_deref(), Some("No message to react to"));
    reset(
        &mut app,
        vec![with(msg(1), |item| item.content = MessageContent::Redacted)],
    );
    assert_eq!(react(&mut app).as_deref(), Some("The message is deleted"));
}

#[test]
fn g_and_a_digit_in_the_composers_normal_mode_go_to_the_editor_and_never_react() {
    let mut app = opened();
    let thumbs = ReactionGroup {
        key: "👍".into(),
        count: 1,
        mine: false,
    };
    reset(
        &mut app,
        vec![with(msg(1), |item| item.reactions = vec![thumbs])],
    );
    keys(&mut app, "i");
    let replay = |text: &str| ui(UiEffect::ReplayKeys(seq(text).keys().to_vec()));
    // `g` waits for no binding here, so edtui gets it and waits for its
    // `gg`: the digit goes to edtui without the keymap.
    assert_eq!(normal(&mut app, "g"), vec![replay("g")]);
    assert_eq!(
        no_timers(keys_in(&mut app, "1", Some(EditorMode::AwaitingChar))),
        vec![replay("1")]
    );
    // A digit in normal mode is no count in the composer either.
    assert_eq!(normal(&mut app, "g2"), vec![replay("g"), replay("2")]);
    assert_eq!(app.focus(), Focus::Composer);
}

/// `R`, bound by default, resends the failed local echo under the cursor.
#[test]
fn capital_r_resends_a_failed_local_echo() {
    let mut app = app();
    keys(&mut app, "<CR>");
    let failed = SendState::Failed {
        error: "boom".into(),
    };
    reset(
        &mut app,
        vec![
            msg(1),
            with(own(local("t1"), "sending"), |item| {
                item.send_state = Some(SendState::Sending);
            }),
            with(own(local("t2"), "failed"), |item| {
                item.send_state = Some(failed);
            }),
        ],
    );
    assert_eq!(
        keys(&mut app, "R"),
        vec![matrix(MatrixEffect::Resend {
            room: id(C),
            txn: TxnId::new("t2"),
        })]
    );
    for up in ["k", "k"] {
        keys(&mut app, up);
        assert_eq!(keys(&mut app, "R"), vec![]);
        assert_eq!(status(&app), Some("Nothing to resend"));
    }
}

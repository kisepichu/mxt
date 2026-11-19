use super::timeline::{event, item, local, msg, opened, remote, reset, status};
use super::*;
use crate::action::FocusMode;
use crate::effect::{Outgoing, OutgoingKind};
use crate::emoji;
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
        room: id(room),
        mode: FocusMode::Insert,
    })
}
fn load_members(room: &str) -> Effect {
    matrix(MatrixEffect::LoadMembers(id(room)))
}
fn clear(room: &str) -> Effect {
    ui(UiEffect::ClearComposer(id(room)))
}
/// The send of `body` from `!c`, the app's `n`-th from 0.
fn send(n: u64, body: RenderedBody, kind: OutgoingKind) -> Effect {
    matrix(MatrixEffect::Send {
        room: id(C),
        id: SendId(n),
        message: Outgoing { body, kind },
    })
}
fn plain(text: &str) -> RenderedBody {
    render_mentions(text, &[])
}
fn submit(app: &mut App, body: &str) -> Vec<Effect> {
    app.update(Msg::Submit {
        room: id(C),
        body: body.into(),
    })
}
fn edited(app: &mut App, room: &str, empty: bool) {
    app.update(Msg::ComposerEdited {
        room: id(room),
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
        room: id(C),
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
    match app.draft(&id(room)) {
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
        vec![ui(UiEffect::RequestSubmit(id(C)))]
    );
    assert_eq!(
        insert(&mut app, "<C-j>"),
        vec![ui(UiEffect::InsertNewline(id(C)))]
    );
    assert_eq!(
        normal(&mut app, "<C-x><C-e>"),
        vec![ui(UiEffect::ExternalEditor(id(C)))]
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
            ui(UiEffect::InsertNewline(id(C))),
            ui(UiEffect::ExternalEditor(id(C))),
        ]
    );
}

#[test]
fn drafts_are_kept_per_room_across_room_switches() {
    let mut app = opened();
    reset(&mut app, vec![msg(1)]);
    assert_eq!(app.draft(&id(C)), None);
    assert_eq!(keys(&mut app, "r"), vec![focus(C), load_members(C)]);
    assert_eq!(app.draft(&id(C)), Some(&Draft::Reply { to: event(1) }));
    edited(&mut app, C, false);

    normal(&mut app, "<Esc>");
    keys(&mut app, "<C-w>hj<CR>");
    assert_eq!(app.open_room(), Some(&id(B)));
    assert_eq!(app.draft(&id(B)), None);
    app.update(Msg::Timeline {
        room: id(B),
        diffs: vec![TimelineDiff::Reset(vec![own(remote(5), "mine")])],
    });
    // The composer of `!b` is empty although the one of `!c` is not.
    keys(&mut app, "e");
    assert_eq!(edit_target(&app, B), Some(&remote(5)));

    normal(&mut app, "<Esc>");
    keys(&mut app, "<C-w>hk<CR>");
    assert_eq!(app.open_room(), Some(&id(C)));
    assert_eq!(app.draft(&id(C)), Some(&Draft::Reply { to: event(1) }));
    assert_eq!(edit_target(&app, B), Some(&remote(5)));
}

#[test]
fn composer_cancel_keeps_a_reply_text_and_clears_an_edit() {
    let mut app = opened();
    reset(&mut app, vec![own(remote(1), "mine"), msg(2)]);
    keys(&mut app, "r");
    edited(&mut app, C, false);
    assert_eq!(normal(&mut app, "<C-c>"), vec![]);
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));

    edited(&mut app, C, true);
    normal(&mut app, "<Esc>");
    keys(&mut app, "ke");
    assert_eq!(edit_target(&app, C), Some(&remote(1)));
    assert_eq!(normal(&mut app, "<C-c>"), vec![clear(C)]);
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));
    // Cleared: another edit can start.
    normal(&mut app, "<Esc>");
    keys(&mut app, "e");
    assert_eq!(edit_target(&app, C), Some(&remote(1)));
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
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));

    edited(&mut app, C, true);
    assert_eq!(
        keys(&mut app, "e"),
        vec![
            focus(C),
            load_members(C),
            ui(UiEffect::LoadComposer {
                room: id(C),
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
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));
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
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));
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
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));

    assert_eq!(
        failed(&mut app, 0),
        vec![ui(UiEffect::LoadComposer {
            room: id(C),
            text: "@Alice hi".into(),
        })]
    );
    assert_eq!(status(&app), Some("Sending the message failed: boom"));
    assert_eq!(app.draft(&id(C)), Some(&Draft::Reply { to: event(1) }));
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
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));
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
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));

    // A reply started in the empty composer stays a reply to its message.
    edited(&mut app, C, true);
    submit(&mut app, "second");
    normal(&mut app, "<Esc>");
    keys(&mut app, "r");
    assert_eq!(failed(&mut app, 1), vec![]);
    assert_eq!(app.draft(&id(C)), Some(&Draft::Reply { to: event(2) }));
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
    assert_eq!(app.draft(&id(B)), None);

    // Nor when its room opens again.
    let back = keys(&mut app, "<C-w>hk<CR>");
    assert_eq!(app.open_room(), Some(&id(C)));
    assert!(
        !back
            .iter()
            .any(|effect| matches!(effect, Effect::Ui(UiEffect::LoadComposer { .. })))
    );
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));
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
            room: id(C),
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
    assert_eq!(app.draft(&id(C)), None);
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
            room: id(C),
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
        vec![ui(UiEffect::RequestSubmit(id(C)))]
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
        vec![ui(UiEffect::RequestSubmit(id(C)))]
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
    assert_eq!(app.confirmation(), Some("Delete this message? (y/n)"));
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
    assert_eq!(app.confirmation(), question);
    assert_eq!(app.timeline_cursor(), Some(1));
    // A key typed after the question answers it.
    assert_eq!(keys(&mut app, "n"), vec![]);
    assert_eq!(app.confirmation(), None);

    let timer = last_timer(&keys(&mut app, "gy"));
    assert_eq!(app.update(Msg::Timeout { id: timer }), vec![]);
    assert_eq!(app.confirmation(), question);
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
    assert_eq!(app.confirmation(), Some("Delete this message? (y/n)"));
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
    assert_eq!(app.confirmation(), question);
    keys(&mut app, "n");
    // `gy` pending: `g` asks, and the `y` fed again is dropped.
    keys(&mut app, "gy");
    let effects = paste(&mut app, "y", None);
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Matrix(_))),
        "{effects:?}"
    );
    assert_eq!(app.confirmation(), question);
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
        assert_eq!(app.confirmation(), Some("Delete this message? (y/n)"));
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
            room: id(room),
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

#[test]
fn the_emoji_picker_lists_reacted_and_common_emoji_first_and_toggles_one() {
    let reacted = |key: &str, mine: bool| ReactionGroup {
        key: key.into(),
        count: 1,
        mine,
    };
    let mut app = opened();
    reset(
        &mut app,
        vec![with(msg(1), |item| {
            // ❤ without its variation selector, as some clients send it.
            item.reactions = vec![
                reacted("🎉", true),
                reacted("👍", false),
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
    let first: Vec<PickerKey> = picker.items[..9].iter().map(|i| i.key.clone()).collect();
    // The common reactions, then every other emoji in `emojis` order.
    let first_other = emoji::all().next().unwrap().as_str();
    assert_eq!(
        first,
        [
            "🎉",
            "\u{2764}",
            "👍",
            "😂",
            "😮",
            "😢",
            "👀",
            "🙏",
            first_other
        ]
        .map(|key| PickerKey::Reaction(key.into()))
    );
    assert_eq!(picker.items[1].label, ":heart: ❤️");
    assert_eq!(picker.items.len(), emoji::all().count());

    // Typed keys go to the query, not to the timeline bindings.
    keys(&mut app, "jk");
    assert_eq!(app.picker().unwrap().query, "jk");
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

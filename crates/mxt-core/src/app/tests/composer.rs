use super::timeline::{event, item, local, msg, opened, remote, reset, status};
use super::*;
use crate::action::FocusMode;
use crate::effect::{Outgoing, OutgoingKind};
use crate::emoji;
use crate::ids::{TxnId, UserId};
use crate::mention::{Member, MentionRef, RenderedBody, render_mentions};
use crate::rich::{Block, Inline, MentionTarget, RichText, Style};
use crate::timeline::{
    Draft, ItemRef, MessageContent, ReactionGroup, SendState, TimelineDiff, TimelineEntry,
    TimelineItem,
};

const C: &str = "!c:example.org";
const B: &str = "!b:example.org";
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
fn send(body: RenderedBody, kind: OutgoingKind) -> Effect {
    matrix(MatrixEffect::Send {
        room: id(C),
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
fn drafts_are_kept_per_room_across_room_switches() {
    let mut app = opened();
    reset(&mut app, vec![msg(1)]);
    assert_eq!(app.draft(&id(C)), None);
    assert_eq!(keys(&mut app, "r"), vec![focus(C), load_members(C)]);
    assert_eq!(app.draft(&id(C)), Some(&Draft::Reply { to: remote(1) }));
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
    assert_eq!(app.draft(&id(C)), Some(&Draft::Reply { to: remote(1) }));
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
fn message_edit_takes_an_own_text_message_into_an_empty_composer_with_its_mentions() {
    let alice = UserId::new(ALICE);
    let content = RichText {
        blocks: vec![Block::Paragraph(vec![
            Inline::Mention {
                text: "@Alice".into(),
                target: MentionTarget::User(alice.clone()),
            },
            Inline::Text {
                text: " hi".into(),
                style: Style::default(),
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
                item.content = MessageContent::Text(content);
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
                text: "@Alice hi".into(),
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
        submit(&mut app, "@Alice hi again"),
        vec![
            send(
                render_mentions("@Alice hi again", &mentions),
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
    reset(&mut app, vec![msg(1), own(local("t1"), "pending")]);
    keys(&mut app, "i");
    assert_eq!(submit(&mut app, " \n "), vec![]);
    assert_eq!(
        submit(&mut app, "hello"),
        vec![send(plain("hello"), OutgoingKind::New), clear(C)]
    );

    // A reply to a local echo waits until the echo has its event ID.
    normal(&mut app, "<Esc>");
    keys(&mut app, "r");
    assert_eq!(app.draft(&id(C)), Some(&Draft::Reply { to: local("t1") }));
    assert_eq!(submit(&mut app, "re"), vec![]);
    assert_eq!(status(&app), Some("Wait until the message is sent"));
    assert_eq!(app.draft(&id(C)), Some(&Draft::Reply { to: local("t1") }));

    normal(&mut app, "<Esc>");
    keys(&mut app, "kr");
    assert_eq!(
        submit(&mut app, "re"),
        vec![
            send(plain("re"), OutgoingKind::Reply { to: event(1) }),
            clear(C),
        ]
    );
    assert_eq!(app.draft(&id(C)), Some(&Draft::New));
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
            send(render_mentions("@Alfred hi", &mentions), OutgoingKind::New),
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
    // Only the focused composer has a menu.
    normal(&mut app, "<Esc>");
    query(&mut app, Some("b"));
    assert_eq!(menu(&app), None);
    keys(&mut app, "i");
    assert_eq!(menu(&app), None);
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
    assert_eq!(app.confirmation(), None);

    // Any other key cancels, and goes nowhere else.
    keys(&mut app, "dd");
    assert_eq!(keys(&mut app, "j"), vec![]);
    assert_eq!(app.confirmation(), None);
    assert_eq!(app.timeline_cursor(), Some(0));
    assert_eq!(app.mode_label(None), "NORMAL");
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
    let first: Vec<&str> = picker.items[..9].iter().map(|i| i.key.as_str()).collect();
    assert_eq!(
        first,
        ["🎉", "\u{2764}", "👍", "😂", "😮", "😢", "👀", "🙏", "😀"]
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
            key: "\u{2764}".into(),
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
fn message_resend_sends_a_failed_local_echo_again() {
    let resend = Binding::single(Action::MessageResend);
    let mut app = app_with(vec![(TableId::Timeline, seq("R"), resend)]);
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

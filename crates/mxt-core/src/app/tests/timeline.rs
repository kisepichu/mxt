use super::*;
use crate::effect::ViewContent;
use crate::ids::{EventId, TxnId, UserId};
use crate::rich::{Block, Inline, RichText, Style};
use crate::timeline::{
    ItemRef, MessageContent, ReplyPreview, TimelineDiff, TimelineEntry, TimelineItem,
};

pub(super) fn event(n: u32) -> EventId {
    EventId::new(format!("${n}:example.org"))
}
pub(super) fn remote(n: u32) -> ItemRef {
    ItemRef::Remote(event(n))
}
pub(super) fn local(txn: &str) -> ItemRef {
    ItemRef::Local(TxnId::new(txn))
}
pub(super) fn item(id: ItemRef, body: &str) -> TimelineEntry {
    TimelineEntry::Item(TimelineItem {
        id,
        sender: UserId::new("@alice:example.org"),
        sender_name: "Alice".into(),
        timestamp: Timestamp::from_millis(0),
        content: MessageContent::Text(RichText::plain(body)),
        reply: None,
        reactions: Vec::new(),
        edited: false,
        send_state: None,
    })
}
/// A remote message with the body `message <n>`.
pub(super) fn msg(n: u32) -> TimelineEntry {
    item(remote(n), &format!("message {n}"))
}
fn msgs(ns: std::ops::Range<u32>) -> Vec<TimelineEntry> {
    ns.map(msg).collect()
}
fn divider() -> TimelineEntry {
    TimelineEntry::DateDivider(Timestamp::from_millis(0))
}
/// The app with `!c` open and the timeline focused.
pub(super) fn opened() -> App {
    let mut app = app();
    keys(&mut app, "<CR>");
    app
}
fn diffs(app: &mut App, diffs: Vec<TimelineDiff>) -> Vec<Effect> {
    app.update(Msg::Timeline {
        room: id("!c:example.org"),
        diffs,
    })
}
pub(super) fn reset(app: &mut App, entries: Vec<TimelineEntry>) -> Vec<Effect> {
    diffs(app, vec![TimelineDiff::Reset(entries)])
}
fn viewport(app: &mut App, first: usize, last: usize) -> Vec<Effect> {
    app.update(Msg::Viewport {
        pane: Pane::Timeline,
        first,
        last,
    })
}
fn done(app: &mut App, reached_start: bool) -> Vec<Effect> {
    app.update(Msg::PaginationDone {
        room: id("!c:example.org"),
        reached_start,
    })
}
fn cursor_body(app: &App) -> String {
    match &app.timeline()[app.timeline_cursor().unwrap()] {
        TimelineEntry::Item(item) => item.content.plain_text(),
        other => panic!("cursor on {other:?}"),
    }
}
fn paginate(room: &str) -> Effect {
    Effect::Matrix(MatrixEffect::PaginateBackwards(id(room)))
}
fn receipt(room: &str, n: u32) -> Effect {
    Effect::Matrix(MatrixEffect::SendReceipt {
        room: id(room),
        event: event(n),
    })
}
pub(super) fn status(app: &App) -> Option<&str> {
    app.status().map(|status| status.text.as_str())
}

#[test]
fn the_cursor_follows_new_items() {
    let mut app = opened();
    assert!(app.timeline_follows());
    assert_eq!(app.timeline_cursor(), None);
    reset(&mut app, vec![divider(), msg(1), msg(2)]);
    assert_eq!(app.timeline_cursor(), Some(2));
    diffs(&mut app, vec![TimelineDiff::PushBack(msg(3))]);
    assert_eq!(cursor_body(&app), "message 3");
    diffs(
        &mut app,
        vec![TimelineDiff::Append(vec![
            TimelineEntry::ReadMarker,
            msg(4),
        ])],
    );
    assert_eq!(app.timeline_cursor(), Some(5));
    assert!(app.timeline_follows());
}

#[test]
fn k_leaves_follow_mode_and_stays_on_its_item() {
    let mut app = opened();
    reset(&mut app, msgs(1..4));
    keys(&mut app, "k");
    assert!(!app.timeline_follows());
    assert_eq!(cursor_body(&app), "message 2");
    diffs(&mut app, vec![TimelineDiff::PushBack(msg(4))]);
    assert_eq!(cursor_body(&app), "message 2");
    // Older items, as back pagination inserts them.
    diffs(
        &mut app,
        vec![
            TimelineDiff::PushFront(msg(0)),
            TimelineDiff::Insert(0, divider()),
        ],
    );
    assert_eq!(app.timeline_cursor(), Some(3));
    assert_eq!(cursor_body(&app), "message 2");
    // The item moves within a batch.
    diffs(
        &mut app,
        vec![TimelineDiff::Remove(3), TimelineDiff::PushBack(msg(2))],
    );
    assert_eq!(app.timeline_cursor(), Some(5));
}

/// The SDK replaces a local echo by removing it and inserting its remote
/// event after the last remote event, in one batch.
#[test]
fn the_cursor_stays_on_a_local_echo_replaced_by_its_remote_event() {
    let mut app = opened();
    reset(
        &mut app,
        vec![
            msg(1),
            item(local("t1"), "hello"),
            item(local("t2"), "later"),
        ],
    );
    keys(&mut app, "k");
    assert_eq!(cursor_body(&app), "hello");
    // Two echoes: the remote event takes the first one's index.
    diffs(
        &mut app,
        vec![
            TimelineDiff::Remove(1),
            TimelineDiff::Insert(1, item(remote(5), "hello")),
        ],
    );
    assert_eq!(app.timeline_cursor(), Some(1));
    assert_eq!(cursor_body(&app), "hello");
    assert!(!app.timeline_follows());
    // The cursor is on the remote event now.
    diffs(&mut app, vec![TimelineDiff::PushFront(divider())]);
    assert_eq!(app.timeline_cursor(), Some(2));
    assert_eq!(cursor_body(&app), "hello");
    // One echo, the last entry: the remote event is pushed back.
    keys(&mut app, "/later<CR>");
    assert_eq!(app.timeline_cursor(), Some(3));
    assert!(!app.timeline_follows());
    diffs(
        &mut app,
        vec![
            TimelineDiff::Remove(3),
            TimelineDiff::PushBack(item(remote(6), "later")),
        ],
    );
    assert_eq!(app.timeline_cursor(), Some(3));
    assert_eq!(cursor_body(&app), "later");
    assert!(!app.timeline_follows());
}

#[test]
fn when_its_item_disappears_the_cursor_moves_to_a_newer_item_or_follows() {
    let mut app = opened();
    reset(&mut app, msgs(1..5));
    keys(&mut app, "2k");
    assert_eq!(cursor_body(&app), "message 2");
    diffs(&mut app, vec![TimelineDiff::Remove(1)]);
    assert_eq!(cursor_body(&app), "message 3");
    // A reset that keeps the item keeps the cursor on it.
    reset(&mut app, vec![msg(0), divider(), msg(1), msg(3), msg(4)]);
    assert_eq!(app.timeline_cursor(), Some(3));
    assert!(!app.timeline_follows());
    diffs(&mut app, vec![TimelineDiff::Truncate(3)]);
    assert!(app.timeline_follows());
    assert_eq!(cursor_body(&app), "message 1");
}

#[test]
fn last_and_down_return_to_follow_mode() {
    let mut app = opened();
    reset(&mut app, msgs(1..5));
    keys(&mut app, "3k");
    assert_eq!(cursor_body(&app), "message 1");
    keys(&mut app, "G");
    assert!(app.timeline_follows());
    assert_eq!(cursor_body(&app), "message 4");
    keys(&mut app, "2kj");
    assert!(!app.timeline_follows());
    keys(&mut app, "j");
    assert!(app.timeline_follows());
    diffs(&mut app, vec![TimelineDiff::PushBack(msg(5))]);
    assert_eq!(cursor_body(&app), "message 5");
}

#[test]
fn dividers_the_read_marker_and_hidden_entries_are_skipped() {
    let mut app = opened();
    reset(
        &mut app,
        vec![
            divider(),
            msg(1),
            TimelineEntry::ReadMarker,
            TimelineEntry::Hidden,
            divider(),
            msg(2),
            TimelineEntry::Hidden,
        ],
    );
    assert_eq!(app.timeline_cursor(), Some(5));
    keys(&mut app, "k");
    assert_eq!(app.timeline_cursor(), Some(1));
    keys(&mut app, "j");
    assert_eq!(app.timeline_cursor(), Some(5));
    keys(&mut app, "gg");
    assert_eq!(app.timeline_cursor(), Some(1));
}

#[test]
fn cursor_keys_move_the_timeline_only_while_it_has_focus() {
    let mut app = opened();
    reset(&mut app, msgs(1..4));
    keys(&mut app, "<C-w>hj");
    assert_eq!(cursor_id(&app), "!b:example.org");
    assert!(app.timeline_follows());
    keys(&mut app, "<C-w>lk");
    assert_eq!(cursor_body(&app), "message 2");
    assert_eq!(cursor_id(&app), "!b:example.org");
}

#[test]
fn half_page_in_the_timeline_counts_the_visible_items() {
    let mut app = opened();
    // Items at 1, 3, 4, 6, 7, 8, 9.
    reset(
        &mut app,
        vec![
            divider(),
            msg(1),
            divider(),
            msg(2),
            msg(3),
            divider(),
            msg(4),
            msg(5),
            msg(6),
            msg(7),
        ],
    );
    // Six entries, four of them items: a half page is two items.
    viewport(&mut app, 2, 7);
    keys(&mut app, "<C-u>");
    assert_eq!(cursor_body(&app), "message 5");
    keys(&mut app, "<C-d>");
    assert!(app.timeline_follows());
}

#[test]
fn pagination_is_requested_near_the_oldest_item_once_while_in_flight() {
    let mut app = opened();
    assert_eq!(reset(&mut app, msgs(0..30)), vec![]);
    // Eleven older items.
    assert_eq!(keys(&mut app, "18k"), vec![]);
    assert_eq!(keys(&mut app, "k"), vec![paginate("!c:example.org")]);
    assert_eq!(keys(&mut app, "k"), vec![]);
    let older = (100..105)
        .map(|n| TimelineDiff::PushFront(msg(n)))
        .collect();
    assert_eq!(diffs(&mut app, older), vec![]);
    assert_eq!(cursor_body(&app), "message 9");
    // Fourteen older items now.
    assert_eq!(done(&mut app, false), vec![]);
    assert_eq!(keys(&mut app, "4k"), vec![paginate("!c:example.org")]);
    assert_eq!(done(&mut app, true), vec![]);
    assert_eq!(no_timers(keys(&mut app, "kgg")), vec![]);
    assert_eq!(
        diffs(&mut app, vec![TimelineDiff::PushBack(msg(30))]),
        vec![]
    );
}

#[test]
fn a_short_timeline_keeps_paginating_until_the_start() {
    let mut app = opened();
    // Nothing to show yet, so the room's history is loaded.
    assert_eq!(reset(&mut app, vec![]), vec![paginate("!c:example.org")]);
    let state = TimelineDiff::PushFront(TimelineEntry::Hidden);
    assert_eq!(diffs(&mut app, vec![state]), vec![]);
    assert_eq!(done(&mut app, false), vec![paginate("!c:example.org")]);
    let older = msgs(0..3)
        .into_iter()
        .map(TimelineDiff::PushFront)
        .collect();
    assert_eq!(diffs(&mut app, older), vec![]);
    assert_eq!(done(&mut app, true), vec![]);
    assert_eq!(keys(&mut app, "k"), vec![]);
    // A reset replaces the loaded history, which may then not reach the start.
    assert_eq!(
        reset(&mut app, msgs(5..8)),
        vec![paginate("!c:example.org")]
    );
}

#[test]
fn a_failed_pagination_shows_the_error_and_allows_a_new_request() {
    let mut app = opened();
    assert_eq!(
        reset(&mut app, msgs(0..3)),
        vec![paginate("!c:example.org")]
    );
    let fx = app.update(Msg::PaginationFailed {
        room: id("!c:example.org"),
        error: "request timed out".into(),
    });
    assert_eq!(fx, vec![]);
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    assert!(status(&app).unwrap().contains("request timed out"));
    assert_eq!(keys(&mut app, "k"), vec![paginate("!c:example.org")]);
}

#[test]
fn the_receipt_is_requested_once_per_newest_remote_event() {
    let mut app = opened();
    // No viewport yet.
    assert_eq!(
        reset(&mut app, msgs(1..3)),
        vec![paginate("!c:example.org")]
    );
    assert_eq!(viewport(&mut app, 0, 1), vec![receipt("!c:example.org", 2)]);
    assert_eq!(
        app.rooms().get(&id("!c:example.org")).unwrap().unread,
        UnreadCounts::default()
    );
    assert_eq!(viewport(&mut app, 0, 1), vec![]);
    // A local echo is not visible yet, then has no event ID.
    let echo = TimelineDiff::PushBack(item(local("t1"), "mine"));
    assert_eq!(diffs(&mut app, vec![echo]), vec![]);
    assert_eq!(viewport(&mut app, 0, 2), vec![]);
    // The SDK replaces the echo by its remote event.
    let sent = vec![
        TimelineDiff::Remove(2),
        TimelineDiff::PushBack(item(remote(3), "mine")),
    ];
    assert_eq!(diffs(&mut app, sent), vec![receipt("!c:example.org", 3)]);
    // The newest item is not visible.
    app.update(Msg::RoomUpserted(room("!c:example.org", 30, 4)));
    assert_eq!(
        diffs(&mut app, vec![TimelineDiff::PushBack(msg(4))]),
        vec![]
    );
    // The terminal is not focused.
    assert_eq!(app.update(Msg::TerminalFocus(false)), vec![]);
    assert_eq!(viewport(&mut app, 1, 3), vec![]);
    assert_eq!(
        app.rooms()
            .get(&id("!c:example.org"))
            .unwrap()
            .unread
            .messages,
        4
    );
    assert_eq!(
        app.update(Msg::TerminalFocus(true)),
        vec![receipt("!c:example.org", 4)]
    );
    assert_eq!(
        app.rooms().get(&id("!c:example.org")).unwrap().unread,
        UnreadCounts::default()
    );
}

#[test]
fn message_view_opens_and_closes() {
    let mut app = opened();
    reset(&mut app, vec![item(remote(1), "first line\nsecond line")]);
    assert_eq!(
        keys(&mut app, "<CR>"),
        vec![Effect::Ui(UiEffect::OpenView(ViewContent::Text(
            "first line\nsecond line".into()
        )))]
    );
    assert!(app.view_open());
    assert_eq!(app.mode_label(Some(EditorMode::Normal)), "VIEW");
    // Unbound keys go to the view's editor; in insert mode, every key does.
    assert_eq!(
        keys_in(&mut app, "j", Some(EditorMode::Normal)),
        vec![Effect::Ui(UiEffect::ReplayKeys(seq("j").keys().to_vec()))]
    );
    assert_eq!(
        keys_in(&mut app, "q", Some(EditorMode::Insert)),
        vec![Effect::Ui(UiEffect::ReplayKeys(seq("q").keys().to_vec()))]
    );
    assert!(app.view_open());
    assert_eq!(
        keys_in(&mut app, "q", Some(EditorMode::Normal)),
        vec![Effect::Ui(UiEffect::CloseView)]
    );
    assert!(!app.view_open());
    assert_eq!(app.mode_label(None), "NORMAL");
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn yy_yields_the_plain_text() {
    let mut app = opened();
    let bold = Style {
        bold: true,
        ..Style::default()
    };
    let mut entry = msg(1);
    if let TimelineEntry::Item(item) = &mut entry {
        item.content = MessageContent::Text(RichText {
            blocks: vec![Block::Paragraph(vec![
                Inline::Text {
                    text: "hello ".into(),
                    style: Style::default(),
                },
                Inline::Text {
                    text: "world".into(),
                    style: bold,
                },
            ])],
        });
    }
    reset(&mut app, vec![entry]);
    assert_eq!(
        no_timers(keys(&mut app, "yy")),
        vec![Effect::Ui(UiEffect::Yank("hello world".into()))]
    );
}

#[test]
fn gd_jumps_to_a_loaded_reply_target_and_reports_a_missing_one() {
    let reply = |n: u32, to: u32| {
        let mut entry = msg(n);
        if let TimelineEntry::Item(item) = &mut entry {
            item.reply = Some(ReplyPreview {
                event: event(to),
                sender_name: None,
                body: None,
            });
        }
        entry
    };
    let mut app = opened();
    reset(&mut app, vec![msg(1), divider(), reply(2, 1), reply(3, 9)]);
    keys(&mut app, "gd");
    assert_eq!(status(&app), Some("The replied-to message is not loaded"));
    assert_eq!(cursor_body(&app), "message 3");
    keys(&mut app, "kgd");
    assert_eq!(app.timeline_cursor(), Some(0));
    assert!(!app.timeline_follows());
    keys(&mut app, "gd");
    assert_eq!(status(&app), Some("The message is not a reply"));
    assert_eq!(app.timeline_cursor(), Some(0));
}

#[test]
fn message_actions_without_an_item_do_nothing() {
    let mut app = opened();
    reset(&mut app, vec![divider(), TimelineEntry::Hidden]);
    assert_eq!(no_timers(keys(&mut app, "<CR>yygdkjG")), vec![]);
    assert!(!app.view_open());
    assert_eq!(app.status(), None);
    assert_eq!(app.timeline_cursor(), None);
}

#[test]
fn timeline_search_wraps() {
    let mut app = opened();
    reset(
        &mut app,
        vec![
            item(remote(1), "an apple"),
            item(remote(2), "banana"),
            divider(),
            item(remote(3), "Apple pie"),
        ],
    );
    // From the newest item, wrapping to the oldest.
    keys(&mut app, "/APPLE<CR>");
    assert_eq!(cursor_body(&app), "an apple");
    assert!(!app.timeline_follows());
    keys(&mut app, "n");
    assert_eq!(cursor_body(&app), "Apple pie");
    keys(&mut app, "N");
    assert_eq!(cursor_body(&app), "an apple");
    keys(&mut app, "/cherry<CR>");
    assert_eq!(status(&app), Some("Pattern not found: cherry"));
    assert_eq!(cursor_body(&app), "an apple");
}

#[test]
fn messages_for_another_room_are_ignored() {
    let mut app = opened();
    let other = app.update(Msg::Timeline {
        room: id("!a:example.org"),
        diffs: vec![TimelineDiff::Reset(msgs(0..3))],
    });
    assert_eq!(other, vec![]);
    assert!(app.timeline().is_empty());
    assert_eq!(
        reset(&mut app, msgs(0..3)),
        vec![paginate("!c:example.org")]
    );
    app.update(Msg::PaginationFailed {
        room: id("!a:example.org"),
        error: "boom".into(),
    });
    app.update(Msg::PaginationDone {
        room: id("!a:example.org"),
        reached_start: true,
    });
    assert_eq!(app.status(), None);
    // Still in flight for `!c`, and its start is not reached.
    assert_eq!(keys(&mut app, "k"), vec![]);
    assert_eq!(done(&mut app, false), vec![paginate("!c:example.org")]);
}

#[test]
fn opening_another_room_resets_the_timeline() {
    let mut app = opened();
    reset(&mut app, msgs(1..3));
    keys(&mut app, "k");
    assert_eq!(viewport(&mut app, 0, 1), vec![receipt("!c:example.org", 2)]);
    keys(&mut app, "<C-w>hj<CR>");
    assert_eq!(app.open_room(), Some(&id("!b:example.org")));
    assert!(app.timeline().is_empty());
    assert_eq!(app.timeline_cursor(), None);
    assert!(app.timeline_follows());
    // Diffs and pagination of the previous room no longer count.
    assert_eq!(reset(&mut app, msgs(1..3)), vec![]);
    assert!(app.timeline().is_empty());
    keys(&mut app, "<C-w>hk<CR>");
    assert_eq!(app.open_room(), Some(&id("!c:example.org")));
    // A new request although the old one never finished, and a new receipt.
    assert_eq!(
        reset(&mut app, vec![msg(0), msg(1), divider(), msg(2)]),
        vec![paginate("!c:example.org")]
    );
    assert!(app.timeline_follows());
    assert_eq!(viewport(&mut app, 0, 3), vec![receipt("!c:example.org", 2)]);
}

#[test]
fn removing_the_open_room_clears_the_timeline_and_closes_the_view() {
    let mut app = opened();
    reset(&mut app, msgs(1..3));
    keys(&mut app, "<CR>");
    assert!(app.view_open());
    let fx = app.update(Msg::RoomRemoved(id("!c:example.org")));
    assert!(fx.contains(&Effect::Ui(UiEffect::CloseView)));
    assert!(!app.view_open());
    assert_eq!(app.mode_label(None), "NORMAL");
    assert!(app.timeline().is_empty());
    assert_eq!(app.timeline_cursor(), None);
}

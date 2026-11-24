//! Threads (design, "Threads"): the timeline messages that count, the
//! thread roots' unread mark, and a thread shown in place of the main
//! timeline.

use std::num::NonZeroU32;

use super::timeline::{
    done, event, item, local, msg, opened, paginate, receipt, remote, reset, status, viewport,
};
use super::*;
use crate::action::FocusMode;
use crate::effect::{Outgoing, OutgoingKind, ViewContent};
use crate::ids::{EventId, SendId, UserId};
use crate::mention::render_mentions;
use crate::timeline::{
    Draft, ItemRef, MessageContent, ReactionGroup, ReactionSender, ReplyPreview, ThreadSummary,
    TimelineDiff, TimelineEntry, TimelineItem,
};

const A: &str = "!a:example.org";
const C: &str = "!c:example.org";
const BOB: &str = "@bob:example.org";

/// The reactions with `key` from `senders`, oldest first, without names.
fn group(key: &str, senders: &[&str], mine: bool) -> ReactionGroup {
    let senders = senders.iter().map(|user| ReactionSender {
        user: UserId::new(*user),
        name: None,
    });
    ReactionGroup {
        key: key.into(),
        senders: senders.collect(),
        mine,
    }
}

/// The thread of `room` whose root is `$<root>`.
fn thread_of(room: &str, root: u32) -> TimelineId {
    TimelineId {
        room: id(room),
        thread: Some(event(root)),
    }
}
/// `entry`, an item, sent at `ms`.
pub(super) fn at(ms: u64, mut entry: TimelineEntry) -> TimelineEntry {
    if let TimelineEntry::Item(item) = &mut entry {
        item.timestamp = Timestamp::from_millis(ms);
    }
    entry
}
fn time(ms: u64) -> Option<Timestamp> {
    Some(Timestamp::from_millis(ms))
}
/// `!c`'s summary with its latest activity at `ms` and `messages` unread,
/// as the SDK reports it.
fn counted(app: &mut App, at: u64, messages: u64) {
    app.update(Msg::RoomUpserted(room(C, at, messages)));
}

#[test]
fn only_the_open_rooms_main_timeline_changes_the_timeline() {
    let mut app = opened();
    for other in [thread_of(C, 1), main_of(A)] {
        let fx = app.update(Msg::Timeline {
            timeline: other.clone(),
            diffs: vec![TimelineDiff::Reset(vec![msg(1)])],
        });
        assert_eq!(fx, vec![], "{other:?}");
        assert!(app.timeline().is_empty(), "{other:?}");
    }
    let fx = app.update(Msg::Timeline {
        timeline: main_of(C),
        diffs: vec![TimelineDiff::Reset(vec![msg(1)])],
    });
    assert_eq!(fx, vec![paginate(C)]);
    assert_eq!(app.timeline(), [msg(1)]);
}

#[test]
fn back_pagination_names_the_main_timeline_and_ignores_a_threads_answers() {
    let mut app = opened();
    let fx = reset(&mut app, (0..3).map(msg).collect());
    assert_eq!(
        fx,
        vec![Effect::Matrix(MatrixEffect::PaginateBackwards(main_of(C)))]
    );
    // Applied, the thread's answer would request the next page at once: the
    // cursor is near the oldest item.
    let fx = app.update(Msg::PaginationDone {
        timeline: thread_of(C, 1),
        reached_start: false,
    });
    assert_eq!(fx, vec![]);
    app.update(Msg::PaginationFailed {
        timeline: thread_of(C, 1),
        error: "boom".into(),
    });
    assert_eq!(status(&app), None);
    // Still in flight for the main timeline, whose start is not loaded.
    assert_eq!(keys(&mut app, "k"), vec![]);
    assert_eq!(done(&mut app, false), vec![paginate(C)]);
}

#[test]
fn thread_marks_follow_what_mxt_marked_read_while_the_room_has_unread_messages() {
    let mark = Binding::single(Action::RoomMarkRead);
    let mut app = app_with(vec![(TableId::Normal, seq("<Space>r"), mark)]);
    keys(&mut app, "<CR>"); // open !c, with 2 unread messages
    let echo = at(3_000, item(local("t1"), "mine"));
    reset(&mut app, vec![at(1_000, msg(1)), at(2_000, msg(2)), echo]);
    // Nothing marked read yet.
    assert_eq!(app.thread_marks_after(), None);

    // The receipt goes on the newest remote event, sent at 2000, and clears
    // the room's counts at once.
    assert_eq!(viewport(&mut app, 0, 2), vec![receipt(C, 2)]);
    assert_eq!(app.thread_marks_after(), None);
    // The SDK counts the replies in threads after the receipt again.
    counted(&mut app, 2_500, 1);
    assert_eq!(app.thread_marks_after(), time(2_000));

    // `room.mark_read` on the open room: up to its latest activity, when
    // that is later.
    counted(&mut app, 1_500, 1);
    keys(&mut app, "<Space>r");
    counted(&mut app, 1_500, 1);
    assert_eq!(app.thread_marks_after(), time(2_000));
    counted(&mut app, 5_000, 1);
    keys(&mut app, "<Space>r");
    assert_eq!(app.thread_marks_after(), None);
    counted(&mut app, 5_000, 1);
    assert_eq!(app.thread_marks_after(), time(5_000));

    // On another room it leaves them, even when that room's latest activity
    // is later.
    app.update(Msg::RoomUpserted(room(A, 9_000, 1)));
    keys(&mut app, "<C-w>hgg<Space>r");
    assert_eq!(app.rooms().rooms()[app.room_cursor().unwrap()].id, id(A));
    assert_eq!(app.thread_marks_after(), time(5_000));

    // A room read elsewhere marks no thread.
    counted(&mut app, 5_000, 0);
    assert_eq!(app.thread_marks_after(), None);
}

/// `entry`, the root of a thread with `replies` replies.
fn with_replies(replies: u32, mut entry: TimelineEntry) -> TimelineEntry {
    if let TimelineEntry::Item(item) = &mut entry {
        item.thread_summary = Some(ThreadSummary {
            replies: NonZeroU32::new(replies).unwrap(),
            latest: None,
        });
    }
    entry
}
/// `message <n>`, deleted.
fn deleted(n: u32) -> TimelineEntry {
    let mut entry = msg(n);
    if let TimelineEntry::Item(item) = &mut entry {
        item.content = MessageContent::Redacted;
    }
    entry
}
/// `message <n>`, a reply to `message <to>`.
fn reply(n: u32, to: u32) -> TimelineEntry {
    let mut entry = msg(n);
    if let TimelineEntry::Item(item) = &mut entry {
        item.reply = Some(ReplyPreview {
            event: event(to),
            sender_name: None,
            body: None,
        });
    }
    entry
}
fn thread_reset(app: &mut App, root: u32, entries: Vec<TimelineEntry>) -> Vec<Effect> {
    app.update(Msg::Timeline {
        timeline: thread_of(C, root),
        diffs: vec![TimelineDiff::Reset(entries)],
    })
}
fn subscribe(root: u32) -> Effect {
    Effect::Matrix(MatrixEffect::SubscribeThread {
        room: id(C),
        root: event(root),
    })
}
fn unsubscribe(root: u32) -> Effect {
    Effect::Matrix(MatrixEffect::UnsubscribeThread {
        room: id(C),
        root: event(root),
    })
}
fn paginate_thread(root: u32) -> Effect {
    Effect::Matrix(MatrixEffect::PaginateBackwards(thread_of(C, root)))
}
/// The receipt of the thread of message 2 on `message <n>`.
fn thread_receipt(n: u32, unthreaded: bool) -> Effect {
    Effect::Matrix(MatrixEffect::SendThreadReceipt {
        room: id(C),
        root: event(2),
        event: event(n),
        unthreaded,
    })
}
/// The remote event under the timeline cursor.
fn cursor_event(app: &App) -> Option<EventId> {
    match &app.timeline()[app.timeline_cursor()?] {
        TimelineEntry::Item(TimelineItem {
            id: ItemRef::Remote(event),
            ..
        }) => Some(event.clone()),
        _ => None,
    }
}
fn on(n: u32) -> Option<EventId> {
    Some(event(n))
}
/// The main timeline of `on_the_root`.
fn main_entries() -> Vec<TimelineEntry> {
    vec![msg(1), with_replies(2, msg(2)), msg(3)]
}
/// The entries of the thread of message 2: the root and the replies 10 to
/// 12.
fn thread_entries() -> Vec<TimelineEntry> {
    vec![msg(2), msg(10), msg(11), msg(12)]
}
/// `!c` open with messages 1 to 3, message 2 the root of a thread, and the
/// cursor on message 2.
fn on_the_root() -> App {
    let mut app = opened();
    reset(&mut app, main_entries());
    keys(&mut app, "k");
    app
}
/// [`on_the_root`], then `t` and the thread's entries; the cursor follows
/// the newest reply.
fn in_the_thread() -> App {
    let mut app = on_the_root();
    keys(&mut app, "t");
    thread_reset(&mut app, 2, thread_entries());
    app
}

#[test]
fn t_shows_the_thread_of_the_message_under_the_cursor_in_place_of_the_timeline() {
    let mut app = on_the_root();
    assert_eq!(keys(&mut app, "t"), vec![subscribe(2)]);
    assert_eq!(app.shown_timeline(), Some(thread_of(C, 2)));
    assert_eq!(app.focus(), Focus::Timeline);
    assert_eq!(app.timeline(), []);
    // The title row has the root from the main timeline until the thread
    // holds it.
    let root = app
        .thread()
        .and_then(|thread| thread.item)
        .map(|item| &item.id);
    assert_eq!(root, Some(&ItemRef::Remote(event(2))));

    assert_eq!(
        thread_reset(&mut app, 2, vec![msg(10), msg(11)]),
        vec![paginate_thread(2)]
    );
    assert_eq!(app.timeline(), [msg(10), msg(11)]);
    assert_eq!(cursor_event(&app), on(11));
    // The timeline's keys move the thread's cursor.
    keys(&mut app, "k");
    assert_eq!(cursor_event(&app), on(10));
    // Another thread's diffs are ignored.
    assert_eq!(thread_reset(&mut app, 3, vec![msg(20)]), vec![]);
    assert_eq!(app.timeline(), [msg(10), msg(11)]);
}

#[test]
fn t_starts_a_thread_on_a_message_without_replies_and_refuses_echoes_and_deleted_messages() {
    let mut app = opened();
    let echo = item(local("t1"), "mine");
    reset(
        &mut app,
        vec![with_replies(1, deleted(1)), deleted(2), msg(3), echo],
    );
    assert_eq!(keys(&mut app, "t"), vec![]);
    assert_eq!(status(&app), Some("Wait until the message is sent"));
    assert_eq!(keys(&mut app, "kkt"), vec![]);
    assert_eq!(status(&app), Some("The message is deleted"));
    assert_eq!(app.shown_timeline(), Some(main_of(C)));

    assert_eq!(keys(&mut app, "jt"), vec![subscribe(3)]);
    assert_eq!(keys(&mut app, "q"), vec![unsubscribe(3)]);
    // A deleted root keeps its replies.
    assert_eq!(keys(&mut app, "kkt"), vec![subscribe(1)]);
}

#[test]
fn t_in_a_thread_keeps_it() {
    let mut app = in_the_thread();
    assert_eq!(keys(&mut app, "kt"), vec![]);
    assert_eq!(app.shown_timeline(), Some(thread_of(C, 2)));
    assert_eq!(cursor_event(&app), on(11));
}

/// The message view of `message <n>`, without search matches.
fn text_view(n: u32) -> Effect {
    Effect::Ui(UiEffect::OpenView(ViewContent::Text {
        text: format!("message {n}"),
        matches: Vec::new(),
    }))
}

#[test]
fn enter_on_a_root_with_replies_opens_its_thread_as_t_does() {
    let mut app = on_the_root();
    assert_eq!(keys(&mut app, "<CR>"), vec![subscribe(2)]);
    assert_eq!(app.shown_timeline(), Some(thread_of(C, 2)));
    assert!(!app.view_open());
    thread_reset(&mut app, 2, thread_entries());
    // A jump: `<C-o>` goes back to the root in the timeline.
    assert_eq!(no_timers(keys(&mut app, "<C-o>")), vec![unsubscribe(2)]);
    assert_eq!(app.shown_timeline(), Some(main_of(C)));
    assert_eq!(cursor_event(&app), on(2));

    // A deleted root keeps its replies.
    let mut app = opened();
    reset(&mut app, vec![with_replies(1, deleted(1)), msg(2)]);
    assert_eq!(keys(&mut app, "k<CR>"), vec![subscribe(1)]);
}

#[test]
fn enter_on_a_message_without_replies_shows_it_and_only_t_starts_a_thread() {
    let mut app = on_the_root();
    assert_eq!(keys(&mut app, "j<CR>"), vec![text_view(3)]);
    assert!(app.view_open());
    assert_eq!(app.shown_timeline(), Some(main_of(C)));
    keys(&mut app, "q");
    assert_eq!(keys(&mut app, "t"), vec![subscribe(3)]);

    // Nothing under the cursor: nothing happens.
    let mut app = opened();
    assert_eq!(keys(&mut app, "<CR>"), vec![]);
    assert!(!app.view_open());
}

#[test]
fn enter_in_a_thread_shows_every_message_the_root_included() {
    let mut app = on_the_root();
    keys(&mut app, "<CR>");
    // The root has its `💬` row in the thread too.
    thread_reset(&mut app, 2, vec![with_replies(2, msg(2)), msg(10), msg(11)]);
    assert_eq!(keys(&mut app, "<CR>"), vec![text_view(11)]);
    keys(&mut app, "q");
    keys(&mut app, "kk");
    assert_eq!(keys(&mut app, "<CR>"), vec![text_view(2)]);
    assert_eq!(app.shown_timeline(), Some(thread_of(C, 2)));
}

#[test]
fn message_enter_bound_in_thread_shows_the_root_where_thread_open_keeps_the_focus() {
    let enter = Binding::single(Action::MessageEnter);
    let mut app = app_with(vec![(TableId::Thread, seq("o"), enter)]);
    keys(&mut app, "<CR>");
    reset(&mut app, main_entries());
    keys(&mut app, "k<CR>");
    thread_reset(&mut app, 2, vec![with_replies(1, msg(2)), msg(10)]);
    keys(&mut app, "k");
    assert_eq!(keys(&mut app, "o"), vec![text_view(2)]);
}

#[test]
fn enter_bound_to_message_view_in_the_timeline_shows_a_root_as_before() {
    let view = Binding::single(Action::MessageView);
    let mut app = app_with(vec![(TableId::Timeline, seq("<CR>"), view)]);
    keys(&mut app, "<CR>");
    reset(&mut app, main_entries());
    assert_eq!(keys(&mut app, "k<CR>"), vec![text_view(2)]);
    assert_eq!(app.shown_timeline(), Some(main_of(C)));
}

#[test]
fn react_with_an_index_in_a_thread_counts_the_reactions_of_the_threads_message() {
    let mut app = opened();
    let reacted = |mut entry: TimelineEntry, reactions: &[ReactionGroup]| {
        if let TimelineEntry::Item(item) = &mut entry {
            item.reactions = reactions.to_vec();
        }
        entry
    };
    let toggle = |key: &str| {
        vec![Effect::Matrix(MatrixEffect::ToggleReaction {
            room: id(C),
            item: ItemRef::Remote(event(11)),
            key: key.into(),
        })]
    };
    let root = || with_replies(2, reacted(msg(2), &[group("👀", &[BOB], false)]));
    reset(
        &mut app,
        vec![
            msg(1),
            root(),
            reacted(msg(3), &[group("😂", &[BOB], false)]),
        ],
    );
    keys(&mut app, "kt");
    // The user's ❤ without the variation selector, another user's ❤️.
    let hearts = [
        group("\u{2764}", &[ME], true),
        group("\u{2764}\u{fe0f}", &[BOB], false),
    ];
    thread_reset(
        &mut app,
        2,
        vec![root(), msg(10), reacted(msg(11), &hearts)],
    );
    // On the newest reply, which the cursor follows: `g2` joins the second
    // chip with its key, `g1` removes the user's own.
    assert_eq!(no_timers(keys(&mut app, "g2")), toggle("\u{2764}\u{fe0f}"));
    assert_eq!(no_timers(keys(&mut app, "g1")), toggle("\u{2764}"));
    keys(&mut app, "k");
    assert_eq!(no_timers(keys(&mut app, "g1")), vec![]);
    assert_eq!(status(&app), Some("The message has no reaction 1"));
}

#[test]
fn the_emoji_picker_in_a_thread_lists_the_reactions_shown_under_the_message_first() {
    let mut app = on_the_root();
    keys(&mut app, "t");
    let mut newest = msg(12);
    if let TimelineEntry::Item(item) = &mut newest {
        item.reactions = vec![group("👀", &[BOB], false), group("👍", &[ME], true)];
    }
    thread_reset(&mut app, 2, vec![msg(2), msg(10), msg(11), newest]);
    keys(&mut app, "+");
    let picker = app.picker().unwrap();
    assert_eq!(
        picker.kind,
        PickerKind::Emoji {
            room: id(C),
            item: remote(12),
        }
    );
    let first: Vec<PickerKey> = picker.items[..3].iter().map(|i| i.key.clone()).collect();
    assert_eq!(
        first,
        ["👀", "👍", "❤️"].map(|key| PickerKey::Reaction(key.into()))
    );
    assert_eq!(
        keys(&mut app, "<C-n><CR>"),
        vec![Effect::Matrix(MatrixEffect::ToggleReaction {
            room: id(C),
            item: remote(12),
            key: "👍".into(),
        })]
    );
}

#[test]
fn q_and_esc_show_the_timeline_again_with_the_cursor_on_the_root() {
    for key in ["q", "<Esc>"] {
        let mut app = in_the_thread();
        assert_eq!(keys(&mut app, key), vec![unsubscribe(2)], "{key}");
        assert_eq!(app.shown_timeline(), Some(main_of(C)));
        assert!(app.thread().is_none());
        assert_eq!(app.timeline(), main_entries());
        assert_eq!(cursor_event(&app), on(2));
        assert!(!app.timeline_follows());
        // Nothing to close in the timeline.
        assert_eq!(keys(&mut app, key), vec![], "{key}");
    }
}

#[test]
fn closing_loads_older_messages_until_the_root_arrives() {
    let mut app = in_the_thread();
    // The SDK rebuilt the timeline meanwhile, without the root, and its page
    // in flight ended.
    assert_eq!(reset(&mut app, vec![msg(3)]), vec![]);
    assert_eq!(done(&mut app, false), vec![]);
    assert_eq!(keys(&mut app, "q"), vec![unsubscribe(2), paginate(C)]);
    assert_eq!(status(&app), Some("Loading older messages…"));
    app.update(Msg::Timeline {
        timeline: main_of(C),
        diffs: vec![TimelineDiff::PushFront(with_replies(2, msg(2)))],
    });
    assert_eq!(cursor_event(&app), on(2));
    assert_eq!(status(&app), None);

    // Its start loads without the root: after the grace period, the
    // cursor follows the newest message.
    let mut app = in_the_thread();
    reset(&mut app, vec![msg(3), msg(4)]);
    done(&mut app, false);
    keys(&mut app, "q");
    let timer = last_timer(&done(&mut app, true));
    assert_eq!(status(&app), Some("Loading older messages…"));
    assert_eq!(app.update(Msg::Timeout { id: timer }), vec![]);
    assert!(app.timeline_follows());
    assert_eq!(cursor_event(&app), on(4));
    assert_eq!(status(&app), None);
}

#[test]
fn the_timelines_diffs_while_a_thread_is_shown_change_the_timeline_that_comes_back() {
    let mut app = in_the_thread();
    let pushed = app.update(Msg::Timeline {
        timeline: main_of(C),
        diffs: vec![TimelineDiff::PushBack(msg(4))],
    });
    assert_eq!(pushed, vec![]);
    assert_eq!(app.timeline(), thread_entries());
    keys(&mut app, "q");
    let mut entries = main_entries();
    entries.push(msg(4));
    assert_eq!(app.timeline(), entries);
    assert_eq!(cursor_event(&app), on(2));
}

#[test]
fn back_pagination_names_the_thread_near_its_oldest_item() {
    let mut app = on_the_root();
    keys(&mut app, "t");
    assert_eq!(
        thread_reset(&mut app, 2, vec![msg(10)]),
        vec![paginate_thread(2)]
    );
    // The timeline's answer goes to it aside; the thread's page is still
    // in flight.
    assert_eq!(done(&mut app, false), vec![]);
    assert_eq!(keys(&mut app, "k"), vec![]);
    let answer = app.update(Msg::PaginationDone {
        timeline: thread_of(C, 2),
        reached_start: false,
    });
    assert_eq!(answer, vec![paginate_thread(2)]);
}

#[test]
fn another_room_and_the_rooms_removal_leave_the_thread_first() {
    let mut app = in_the_thread();
    assert_eq!(
        no_timers(keys(&mut app, ":open a:ex<CR>")),
        vec![
            unsubscribe(2),
            Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id(C))),
            Effect::Matrix(MatrixEffect::SubscribeTimeline(id(A))),
        ]
    );
    assert!(app.thread().is_none());
    assert_eq!(app.shown_timeline(), Some(main_of(A)));

    let mut app = in_the_thread();
    assert_eq!(
        app.update(Msg::RoomRemoved(id(C))),
        vec![
            unsubscribe(2),
            Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id(C))),
        ]
    );
    assert!(app.thread().is_none());
    assert_eq!(app.shown_timeline(), None);
}

#[test]
fn each_thread_keeps_its_own_draft_and_sends_into_the_thread() {
    let normal = Some(EditorMode::Normal);
    let mut app = on_the_root();
    // A reply to message 1 in the room's composer, left with its text.
    keys(&mut app, "kr");
    app.update(Msg::ComposerEdited {
        timeline: main_of(C),
        empty: false,
    });
    keys_in(&mut app, "<Esc>", normal);
    keys(&mut app, "jt");
    thread_reset(&mut app, 2, thread_entries());
    // The thread's composer; the room's members are requested already.
    let focus = |timeline: TimelineId| {
        Effect::Ui(UiEffect::FocusComposer {
            timeline,
            mode: FocusMode::Insert,
        })
    };
    assert_eq!(keys(&mut app, "i"), vec![focus(thread_of(C, 2))]);
    app.update(Msg::ComposerEdited {
        timeline: thread_of(C, 2),
        empty: false,
    });
    keys_in(&mut app, "<Esc>", normal);
    keys(&mut app, "q");
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::Reply { to: event(1) }));
    assert_eq!(app.draft(&thread_of(C, 2)), Some(&Draft::New));
    assert_eq!(keys(&mut app, "i"), vec![focus(main_of(C))]);

    keys_in(&mut app, "<Esc>", normal);
    keys(&mut app, "t");
    assert_eq!(keys(&mut app, "i"), vec![focus(thread_of(C, 2))]);
    let sent = app.update(Msg::Submit {
        timeline: thread_of(C, 2),
        body: "in the thread".into(),
    });
    let send = MatrixEffect::Send {
        timeline: thread_of(C, 2),
        id: SendId(0),
        message: Outgoing {
            body: render_mentions("in the thread", &[]),
            kind: OutgoingKind::New,
        },
    };
    assert_eq!(
        sent,
        vec![
            Effect::Matrix(send),
            Effect::Ui(UiEffect::ClearComposer(thread_of(C, 2))),
        ]
    );
    assert_eq!(app.draft(&main_of(C)), Some(&Draft::Reply { to: event(1) }));
}

#[test]
fn upload_in_a_thread_sends_into_it() {
    let mut app = in_the_thread();
    assert_eq!(
        no_timers(keys(&mut app, ":upload /tmp/a.png<CR>")),
        vec![Effect::Matrix(MatrixEffect::Upload {
            timeline: thread_of(C, 2),
            path: "/tmp/a.png".into(),
        })]
    );
}

/// `!c` open with messages 1 to 3 sent at 1000, 2000 and `newest`, the
/// cursor on message 2.
fn timed_root(newest: u64) -> App {
    let mut app = opened();
    reset(
        &mut app,
        vec![
            at(1_000, msg(1)),
            at(2_000, with_replies(1, msg(2))),
            at(newest, msg(3)),
        ],
    );
    app
}
/// `t` on message 2 and the thread's root and reply 10, sent at 4000.
fn timed_thread(app: &mut App) {
    keys(app, "kt");
    thread_reset(app, 2, vec![at(2_000, msg(2)), at(4_000, msg(10))]);
}

#[test]
fn a_threads_receipt_is_unthreaded_when_the_timeline_is_read_and_older() {
    let mut app = timed_root(3_000);
    assert_eq!(viewport(&mut app, 0, 2), vec![receipt(C, 3)]);
    // The SDK counts the reply after the receipt again.
    counted(&mut app, 4_000, 1);
    timed_thread(&mut app);
    assert_eq!(app.thread_marks_after(), None);
    assert_eq!(viewport(&mut app, 0, 1), vec![thread_receipt(10, true)]);
    assert_eq!(
        app.rooms().get(&id(C)).unwrap().unread,
        UnreadCounts::default()
    );
    // Once per event.
    assert_eq!(viewport(&mut app, 1, 1), vec![]);
    keys(&mut app, "q");
    counted(&mut app, 4_000, 1);
    assert_eq!(app.thread_marks_after(), time(4_000));
}

#[test]
fn a_threads_receipt_is_threaded_while_the_timeline_is_unread_or_newer() {
    // The pane never showed the timeline's newest message.
    let mut app = timed_root(3_000);
    timed_thread(&mut app);
    assert_eq!(viewport(&mut app, 0, 1), vec![thread_receipt(10, false)]);
    assert_eq!(app.rooms().get(&id(C)).unwrap().unread.messages, 2);
    keys(&mut app, "q");
    assert_eq!(app.thread_marks_after(), None);

    // The timeline is read, but to a newer message than the reply.
    let mut app = timed_root(5_000);
    assert_eq!(viewport(&mut app, 0, 2), vec![receipt(C, 3)]);
    counted(&mut app, 5_000, 1);
    timed_thread(&mut app);
    assert_eq!(viewport(&mut app, 0, 1), vec![thread_receipt(10, false)]);
    assert_eq!(app.rooms().get(&id(C)).unwrap().unread.messages, 1);
    keys(&mut app, "q");
    assert_eq!(app.thread_marks_after(), time(5_000));
}

#[test]
fn a_reply_with_the_unthreaded_receipt_gets_no_threaded_one_when_its_thread_opens_again() {
    let mut app = timed_root(3_000);
    viewport(&mut app, 0, 2);
    timed_thread(&mut app);
    assert_eq!(viewport(&mut app, 0, 1), vec![thread_receipt(10, true)]);
    keys(&mut app, "q");
    // A newer message of the timeline is read.
    app.update(Msg::Timeline {
        timeline: main_of(C),
        diffs: vec![TimelineDiff::PushBack(at(5_000, msg(4)))],
    });
    assert_eq!(viewport(&mut app, 0, 3), vec![receipt(C, 4)]);
    keys(&mut app, "t");
    thread_reset(&mut app, 2, vec![at(2_000, msg(2)), at(4_000, msg(10))]);
    assert_eq!(viewport(&mut app, 0, 1), vec![]);
    // A newer reply gets its receipt.
    app.update(Msg::Timeline {
        timeline: thread_of(C, 2),
        diffs: vec![TimelineDiff::PushBack(at(4_500, msg(11)))],
    });
    assert_eq!(viewport(&mut app, 0, 2), vec![thread_receipt(11, false)]);
}

#[test]
fn a_thread_without_replies_sends_no_receipt_for_its_root() {
    let mut app = on_the_root();
    keys(&mut app, "t");
    thread_reset(&mut app, 2, vec![msg(2)]);
    assert_eq!(viewport(&mut app, 0, 0), vec![]);
}

#[test]
fn mark_read_in_a_thread_marks_the_timeline_read() {
    let mark = Binding::single(Action::RoomMarkRead);
    let mut app = app_with(vec![(TableId::Normal, seq("<Space>r"), mark)]);
    keys(&mut app, "<CR>");
    reset(&mut app, main_entries());
    keys(&mut app, "kt");
    counted(&mut app, 6_000, 1);
    keys(&mut app, "<Space>r");
    keys(&mut app, "q");
    counted(&mut app, 6_000, 1);
    assert_eq!(app.thread_marks_after(), time(6_000));
}

#[test]
fn opening_and_closing_a_thread_are_jumps() {
    let mut app = in_the_thread();
    keys(&mut app, "k");
    // Back to the position before `t`: the thread closes, the cursor on
    // the root.
    assert_eq!(no_timers(keys(&mut app, "<C-o>")), vec![unsubscribe(2)]);
    assert_eq!(app.shown_timeline(), Some(main_of(C)));
    assert_eq!(cursor_event(&app), on(2));
    // Forward: the thread opens again and loads the reply.
    assert_eq!(
        no_timers(keys(&mut app, "<Tab>")),
        vec![subscribe(2), paginate_thread(2)]
    );
    assert_eq!(status(&app), Some("Loading older messages…"));
    thread_reset(&mut app, 2, thread_entries());
    assert_eq!(cursor_event(&app), on(11));
    assert_eq!(status(&app), None);

    // `q` is a jump too.
    keys(&mut app, "q");
    assert_eq!(cursor_event(&app), on(2));
    assert_eq!(
        no_timers(keys(&mut app, "<C-o>")),
        vec![subscribe(2), paginate_thread(2)]
    );
    thread_reset(&mut app, 2, thread_entries());
    assert_eq!(
        (app.shown_timeline(), cursor_event(&app)),
        (Some(thread_of(C, 2)), on(11))
    );
}

#[test]
fn a_jump_to_a_message_gone_from_the_timeline_aside_is_skipped_at_once() {
    let mut app = opened();
    reset(&mut app, main_entries());
    done(&mut app, true);
    // Entries on message 3 and on the root.
    keys(&mut app, "ggjt");
    thread_reset(&mut app, 2, thread_entries());
    // The root goes from the timeline aside, whose start is loaded.
    app.update(Msg::Timeline {
        timeline: main_of(C),
        diffs: vec![TimelineDiff::Remove(1)],
    });
    assert_eq!(no_timers(keys(&mut app, "<C-o>")), vec![unsubscribe(2)]);
    assert_eq!(
        (app.shown_timeline(), cursor_event(&app)),
        (Some(main_of(C)), on(3))
    );
    assert_eq!(
        status(&app),
        Some("Skipped a jump to a message that is gone")
    );
}

#[test]
fn a_jump_into_a_thread_of_another_room_opens_the_room_and_the_thread() {
    let mut app = in_the_thread();
    keys(&mut app, "k:open a:ex<CR>");
    assert_eq!(
        no_timers(keys(&mut app, "<C-o>")),
        vec![
            Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id(A))),
            Effect::Matrix(MatrixEffect::SubscribeTimeline(id(C))),
            subscribe(2),
            paginate_thread(2),
        ]
    );
    assert_eq!(app.shown_timeline(), Some(thread_of(C, 2)));
    // The room's timeline goes aside.
    assert_eq!(reset(&mut app, main_entries()), vec![]);
    assert_eq!(app.timeline(), []);
    thread_reset(&mut app, 2, thread_entries());
    assert_eq!(cursor_event(&app), on(11));
    assert_eq!(status(&app), None);
}

#[test]
fn gd_in_a_thread_goes_to_a_message_of_the_timeline() {
    let mut app = on_the_root();
    keys(&mut app, "t");
    thread_reset(&mut app, 2, vec![msg(2), reply(10, 1)]);
    assert_eq!(no_timers(keys(&mut app, "gd")), vec![unsubscribe(2)]);
    assert_eq!(app.shown_timeline(), Some(main_of(C)));
    assert_eq!(cursor_event(&app), on(1));
    // One jump: back into the thread, to the reply.
    assert_eq!(
        no_timers(keys(&mut app, "<C-o>")),
        vec![subscribe(2), paginate_thread(2)]
    );
    thread_reset(&mut app, 2, vec![msg(2), reply(10, 1)]);
    assert_eq!(cursor_event(&app), on(10));
}

#[test]
fn gd_to_a_message_in_a_thread_opens_that_thread() {
    let mut app = opened();
    reset(
        &mut app,
        vec![msg(1), with_replies(1, msg(2)), reply(3, 20)],
    );
    let find = Effect::Matrix(MatrixEffect::FindThread {
        room: id(C),
        event: event(20),
    });
    assert_eq!(no_timers(keys(&mut app, "gd")), vec![find]);
    assert_eq!(status(&app), None);
    let found = app.update(Msg::ThreadFound {
        room: id(C),
        event: event(20),
        root: Some(event(2)),
    });
    assert_eq!(found, vec![subscribe(2), paginate_thread(2)]);
    assert_eq!(status(&app), Some("Loading older messages…"));
    thread_reset(&mut app, 2, vec![msg(2), msg(20), msg(21)]);
    assert_eq!(
        (app.shown_timeline(), cursor_event(&app)),
        (Some(thread_of(C, 2)), on(20))
    );
    assert_eq!(status(&app), None);
    // One jump recorded.
    keys(&mut app, "<C-o>");
    assert_eq!(
        (app.shown_timeline(), cursor_event(&app)),
        (Some(main_of(C)), on(3))
    );
    keys(&mut app, "<C-o>");
    assert_eq!(status(&app), Some("No older jump"));
}

#[test]
fn gd_in_a_thread_to_a_message_in_another_thread_opens_that_thread() {
    let mut app = opened();
    reset(
        &mut app,
        vec![msg(1), with_replies(1, msg(2)), with_replies(1, msg(3))],
    );
    keys(&mut app, "kt");
    thread_reset(&mut app, 2, vec![msg(2), reply(10, 30)]);
    let find = Effect::Matrix(MatrixEffect::FindThread {
        room: id(C),
        event: event(30),
    });
    assert_eq!(no_timers(keys(&mut app, "gd")), vec![find]);
    let found = app.update(Msg::ThreadFound {
        room: id(C),
        event: event(30),
        root: Some(event(3)),
    });
    assert_eq!(
        found,
        vec![unsubscribe(2), subscribe(3), paginate_thread(3)]
    );
    assert_eq!(status(&app), Some("Loading older messages…"));
    thread_reset(&mut app, 3, vec![msg(3), msg(30), msg(31)]);
    assert_eq!(
        (app.shown_timeline(), cursor_event(&app)),
        (Some(thread_of(C, 3)), on(30))
    );
    assert_eq!(status(&app), None);
    // Back to the reply in the first thread.
    assert_eq!(
        no_timers(keys(&mut app, "<C-o>")),
        vec![unsubscribe(3), subscribe(2), paginate_thread(2)]
    );
}

#[test]
fn gd_ignores_answers_to_other_or_dropped_questions() {
    let mut app = opened();
    reset(
        &mut app,
        vec![msg(1), with_replies(1, msg(2)), reply(3, 20)],
    );
    let found = |room: &str, n: u32, root: Option<u32>| Msg::ThreadFound {
        room: id(room),
        event: event(n),
        root: root.map(event),
    };
    keys(&mut app, "gd");
    assert_eq!(app.update(found(A, 20, Some(2))), vec![]);
    assert_eq!(app.update(found(C, 21, Some(2))), vec![]);
    // A key drops the question.
    keys(&mut app, "k");
    assert_eq!(app.update(found(C, 20, Some(2))), vec![]);
    assert_eq!(app.shown_timeline(), Some(main_of(C)));
    assert_eq!(status(&app), None);

    // In the shown thread, the message is not loaded.
    keys(&mut app, "t");
    thread_reset(&mut app, 2, vec![msg(2), reply(10, 30)]);
    keys(&mut app, "gd");
    assert_eq!(app.update(found(C, 30, Some(2))), vec![]);
    assert_eq!(status(&app), Some("The replied-to message is not loaded"));
    assert_eq!(app.shown_timeline(), Some(thread_of(C, 2)));
}

#[test]
fn q_and_esc_close_only_a_thread_so_a_normal_binding_applies_in_the_timeline() {
    let quit = Binding::single(Action::AppQuit);
    let mut app = app_with(vec![(TableId::Normal, seq("q"), quit)]);
    keys(&mut app, "<CR>");
    reset(&mut app, vec![msg(1), with_replies(1, msg(2))]);
    assert_eq!(keys(&mut app, "q"), vec![Effect::Ui(UiEffect::Quit)]);
    assert_eq!(keys(&mut app, "t"), vec![subscribe(2)]);
    assert_eq!(keys(&mut app, "q"), vec![unsubscribe(2)]);
}

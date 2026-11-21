//! Threads in the main timeline (design, "Threads"): the timeline messages
//! that count, and the thread roots' unread mark.

use super::timeline::{
    done, event, item, local, msg, opened, paginate, receipt, reset, status, viewport,
};
use super::*;
use crate::timeline::{TimelineDiff, TimelineEntry};

const A: &str = "!a:example.org";
const C: &str = "!c:example.org";

/// The thread of `room` whose root is `$<root>`.
fn thread_of(room: &str, root: u32) -> TimelineId {
    TimelineId {
        room: id(room),
        thread: Some(event(root)),
    }
}
/// `entry`, an item, sent at `ms`.
fn at(ms: u64, mut entry: TimelineEntry) -> TimelineEntry {
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

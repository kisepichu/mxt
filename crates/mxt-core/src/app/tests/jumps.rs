//! The jump list (design, "Jump list").

use super::timeline::{event, msg, status};
use super::*;
use crate::mention::Member;
use crate::msg::JoinedHow;
use crate::timeline::{ReplyPreview, TimelineDiff, TimelineEntry};

const A: &str = "!a:example.org";
const B: &str = "!b:example.org";
const C: &str = "!c:example.org";

/// `message <n>` for each `n`.
fn msgs(ns: std::ops::RangeInclusive<u32>) -> Vec<TimelineEntry> {
    ns.map(msg).collect()
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
fn load(app: &mut App, room: &str, entries: Vec<TimelineEntry>) -> Vec<Effect> {
    app.update(Msg::Timeline {
        room: id(room),
        diffs: vec![TimelineDiff::Reset(entries)],
    })
}
fn start_loaded(app: &mut App, room: &str) -> Vec<Effect> {
    app.update(Msg::PaginationDone {
        room: id(room),
        reached_start: true,
    })
}
fn paginate(room: &str) -> Effect {
    Effect::Matrix(MatrixEffect::PaginateBackwards(id(room)))
}
/// `!c` open with `message 1` to `message <n>`, the cursor following the
/// newest.
fn opened_with(n: u32) -> App {
    let mut app = app();
    keys(&mut app, "<CR>");
    load(&mut app, C, msgs(1..=n));
    app
}
/// The text of the message under the timeline cursor.
fn cursor_body(app: &App) -> Option<String> {
    match &app.timeline()[app.timeline_cursor()?] {
        TimelineEntry::Item(item) => Some(item.content.plain_text()),
        _ => None,
    }
}
fn body(n: u32) -> Option<String> {
    Some(format!("message {n}"))
}
fn open_room(app: &App) -> &str {
    app.open_room().map_or("", RoomId::as_str)
}
/// The entries `!a` alone, message 4 of `!c` and `!b` alone, and the jump
/// back to the second from `!b`: `!c` open, its message 4 not loaded yet.
fn waiting_for_message_4() -> App {
    let mut app = app();
    keys(&mut app, ":open a:ex<CR>:open c:ex<CR>");
    load(&mut app, C, msgs(1..=5));
    keys(&mut app, "k:open b:ex<CR><C-o>");
    assert_eq!(open_room(&app), C);
    assert_eq!(status(&app), Some("Loading older messages…"));
    app
}
/// `!c`'s start loads without message 4; the grace period's timer.
fn start_loaded_without_message_4(app: &mut App) -> TimerId {
    load(app, C, vec![msg(1), msg(2), msg(3), msg(5)]);
    last_timer(&start_loaded(app, C))
}

#[test]
fn back_and_forward_go_between_rooms_and_load_their_messages() {
    let mut app = opened_with(5);
    keys(&mut app, "kk");
    // `<CR>` in the room list opens `!b`: a jump from message 3 of `!c`.
    keys(&mut app, "<C-w>hj<CR>");
    assert_eq!(open_room(&app), B);
    load(&mut app, B, msgs(11..=13));
    keys(&mut app, "k");

    // `!c` opens anew, so its message loads first.
    assert_eq!(
        no_timers(keys(&mut app, "<C-o>")),
        vec![
            Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id(B))),
            Effect::Matrix(MatrixEffect::SubscribeTimeline(id(C))),
            paginate(C),
        ]
    );
    assert_eq!(app.focus(), Focus::Timeline);
    assert_eq!(status(&app), Some("Loading older messages…"));
    load(&mut app, C, msgs(1..=5));
    assert_eq!(cursor_body(&app), body(3));
    assert!(!app.timeline_follows());
    assert_eq!(status(&app), None);

    // `jump.back` from the newest position added it: forward returns there.
    keys(&mut app, "<Tab>");
    load(&mut app, B, msgs(11..=13));
    assert_eq!((open_room(&app), cursor_body(&app)), (B, body(12)));
    keys(&mut app, "<Tab>");
    assert_eq!(status(&app), Some("No newer jump"));
    assert_eq!((open_room(&app), cursor_body(&app)), (B, body(12)));
}

#[test]
fn every_way_of_opening_another_room_records_a_jump() {
    let mut app = app();
    keys(&mut app, "<CR>");
    // The next unread room after `!c`.
    keys(&mut app, "]u");
    assert_eq!(open_room(&app), A);
    keys(&mut app, ":open b:ex<CR>");
    assert_eq!(open_room(&app), B);
    keys(&mut app, "<Space><Space>c:ex<CR>");
    assert_eq!(open_room(&app), C);
    // Opening the open room again is no jump.
    keys(&mut app, "<C-w>hgg<CR>");
    for room in [B, A, C] {
        keys(&mut app, "<C-o>");
        assert_eq!(open_room(&app), room);
        assert_eq!(status(&app), None);
    }
    keys(&mut app, "<C-o>");
    assert_eq!(status(&app), Some("No older jump"));
    assert_eq!(open_room(&app), C);
}

#[test]
fn join_records_a_jump_whether_its_room_opens_at_once_or_on_the_answer() {
    // A room ID of a listed room opens at once.
    let mut app = opened_with(3);
    keys(&mut app, ":join !b:example.org<CR>");
    assert_eq!(open_room(&app), B);
    keys(&mut app, "<C-o>");
    load(&mut app, C, msgs(1..=3));
    assert_eq!((open_room(&app), cursor_body(&app)), (C, body(3)));
    // The answered room opens without a key of its own, through the same
    // place.
    keys(&mut app, "k:join #a:example.org<CR>");
    app.update(Msg::Joined {
        target: "#a:example.org".into(),
        room: id(A),
        how: JoinedHow::Joined,
    });
    assert_eq!(open_room(&app), A);
    keys(&mut app, "<C-o>");
    load(&mut app, C, msgs(1..=3));
    assert_eq!((open_room(&app), cursor_body(&app)), (C, body(2)));
}

#[test]
fn gg_g_searches_and_gd_record_jumps_and_line_moves_do_not() {
    let mut app = app();
    keys(&mut app, "<CR>");
    let mut entries = msgs(1..=20);
    entries.push(reply(21, 8));
    load(&mut app, C, entries);
    // From 21 to 1, then to 6 without a jump.
    keys(&mut app, "gg5j<C-u>j");
    assert_eq!(cursor_body(&app), body(6));
    keys(&mut app, "/message 12<CR>");
    keys(&mut app, "G");
    keys(&mut app, "gd");
    assert_eq!(cursor_body(&app), body(8));
    keys(&mut app, "N");
    assert_eq!(cursor_body(&app), body(12));
    for n in [8, 21, 12, 6, 21] {
        keys(&mut app, "<C-o>");
        assert_eq!(cursor_body(&app), body(n));
    }
    keys(&mut app, "<C-o>");
    assert_eq!(status(&app), Some("No older jump"));
    assert_eq!(cursor_body(&app), body(21));
    for n in [6, 12, 21, 8, 12] {
        keys(&mut app, "<Tab>");
        assert_eq!(cursor_body(&app), body(n));
    }
    keys(&mut app, "<Tab>");
    assert_eq!(status(&app), Some("No newer jump"));
}

#[test]
fn a_jump_after_going_back_drops_the_newer_entries() {
    let mut app = opened_with(10);
    // Entries from 10, 1 and 10; the cursor on 1.
    keys(&mut app, "ggGgg");
    keys(&mut app, "<C-o><C-o>");
    assert_eq!(cursor_body(&app), body(1));
    // A jump from 6 drops the entries after the 1.
    keys(&mut app, "5jG");
    for n in [6, 1, 10] {
        keys(&mut app, "<C-o>");
        assert_eq!(cursor_body(&app), body(n));
    }
    keys(&mut app, "<C-o>");
    assert_eq!(status(&app), Some("No older jump"));
    for n in [1, 6, 10] {
        keys(&mut app, "<Tab>");
        assert_eq!(cursor_body(&app), body(n));
    }
    keys(&mut app, "<Tab>");
    assert_eq!(status(&app), Some("No newer jump"));
}

#[test]
fn a_position_equal_to_the_newest_entry_is_not_added_again() {
    let mut app = opened_with(10);
    // Both from message 10, and so is the current position `<C-o>` adds.
    keys(&mut app, "GG");
    keys(&mut app, "<C-o>");
    assert_eq!(status(&app), Some("No older jump"));
    keys(&mut app, "ggG");
    for n in [1, 10] {
        keys(&mut app, "<C-o>");
        assert_eq!(cursor_body(&app), body(n));
    }
    keys(&mut app, "<C-o>");
    assert_eq!(status(&app), Some("No older jump"));
}

#[test]
fn the_list_keeps_the_newest_100_entries() {
    let mut app = opened_with(10);
    // 150 entries, from 10 and 1 in turn.
    for _ in 0..75 {
        keys(&mut app, "ggG");
    }
    // The current position, added by the first `<C-o>`, is one of the 100.
    for step in 1..=99 {
        keys(&mut app, "<C-o>");
        assert_eq!(status(&app), None, "step {step}");
    }
    keys(&mut app, "<C-o>");
    assert_eq!(status(&app), Some("No older jump"));
}

#[test]
fn a_room_left_is_skipped_and_the_status_line_says_so() {
    let mut app = app();
    keys(&mut app, "<CR>:open a:ex<CR>:open b:ex<CR>");
    app.update(Msg::RoomRemoved(id(A)));
    keys(&mut app, "<C-o>");
    assert_eq!(open_room(&app), C);
    assert_eq!(status(&app), Some("Skipped a jump to a room you left"));
    keys(&mut app, "<Tab>");
    assert_eq!(open_room(&app), B);
    assert_eq!(status(&app), Some("Skipped a jump to a room you left"));
    // Nothing moves when every older entry is skipped.
    app.update(Msg::RoomRemoved(id(C)));
    keys(&mut app, "<C-o>");
    assert_eq!(
        status(&app),
        Some("Skipped 2 jumps to rooms you left; no older jump")
    );
    assert_eq!(open_room(&app), B);
}

#[test]
fn a_message_gone_from_its_loaded_room_is_skipped() {
    let mut app = opened_with(10);
    start_loaded(&mut app, C);
    // Entries from 10, 4 and 6.
    keys(&mut app, "gg3jgg5jG");
    app.update(Msg::Timeline {
        room: id(C),
        diffs: vec![TimelineDiff::Remove(5)],
    });
    keys(&mut app, "<C-o>");
    assert_eq!(cursor_body(&app), body(4));
    assert_eq!(
        status(&app),
        Some("Skipped a jump to a message that is gone")
    );
}

#[test]
fn a_message_not_there_once_its_rooms_start_loads_is_skipped() {
    let mut app = opened_with(5);
    keys(&mut app, "k");
    keys(&mut app, ":open b:ex<CR>");
    load(&mut app, B, msgs(11..=13));
    keys(&mut app, "k");
    keys(&mut app, ":open a:ex<CR><C-o>");
    assert_eq!(open_room(&app), B);
    assert_eq!(status(&app), Some("Loading older messages…"));
    // Message 12 is not there when `!b`'s start is loaded.
    load(&mut app, B, vec![msg(11), msg(13)]);
    let grace = last_timer(&start_loaded(&mut app, B));
    // Its page's diffs may still come during the grace period.
    assert_eq!(open_room(&app), B);
    assert_eq!(status(&app), Some("Loading older messages…"));
    let fx = app.update(Msg::Timeout { id: grace });
    assert_eq!(open_room(&app), C);
    assert!(fx.contains(&paginate(C)), "{fx:?}");
    assert_eq!(
        status(&app),
        Some("Skipped a jump to a message that is gone; loading older messages…")
    );
    load(&mut app, C, msgs(1..=5));
    assert_eq!(cursor_body(&app), body(4));
    assert_eq!(
        status(&app),
        Some("Skipped a jump to a message that is gone")
    );
}

#[test]
fn with_no_entry_left_after_a_gone_message_its_room_stays_open() {
    let mut app = opened_with(5);
    keys(&mut app, "k");
    keys(&mut app, ":open b:ex<CR><C-o>");
    load(&mut app, C, msgs(1..=3));
    let grace = last_timer(&start_loaded(&mut app, C));
    app.update(Msg::Timeout { id: grace });
    assert_eq!(
        status(&app),
        Some("Skipped a jump to a message that is gone; no older jump")
    );
    assert_eq!(open_room(&app), C);
    assert!(app.timeline_follows());
}

#[test]
fn a_message_whose_page_comes_after_the_start_is_reported_still_gets_the_cursor() {
    let mut app = opened_with(5);
    keys(&mut app, "kkk:open b:ex<CR><C-o>");
    load(&mut app, C, msgs(3..=5));
    // The SDK may report the room's start before the diffs of its last page.
    let grace = last_timer(&start_loaded(&mut app, C));
    assert_eq!(open_room(&app), C);
    assert_eq!(status(&app), Some("Loading older messages…"));
    // A change without the message looks again and keeps the grace period.
    let fx = app.update(Msg::Timeline {
        room: id(C),
        diffs: vec![TimelineDiff::PushBack(msg(6))],
    });
    assert_eq!(no_timers(fx.clone()), fx);
    assert_eq!(status(&app), Some("Loading older messages…"));
    app.update(Msg::Timeline {
        room: id(C),
        diffs: vec![
            TimelineDiff::PushFront(msg(2)),
            TimelineDiff::PushFront(msg(1)),
        ],
    });
    assert_eq!((open_room(&app), cursor_body(&app)), (C, body(2)));
    assert_eq!(status(&app), None);
    // The grace period ended with the wait.
    assert_eq!(app.update(Msg::Timeout { id: grace }), vec![]);
    assert_eq!((open_room(&app), cursor_body(&app)), (C, body(2)));
    assert_eq!(status(&app), None);
}

#[test]
fn a_key_or_a_failed_load_ends_the_wait_for_a_message() {
    let mut app = opened_with(5);
    keys(&mut app, "k");
    keys(&mut app, ":open b:ex<CR><C-o>j");
    load(&mut app, C, msgs(1..=5));
    assert!(app.timeline_follows());
    assert_eq!(cursor_body(&app), body(5));

    keys(&mut app, "k:open b:ex<CR><C-o>");
    app.update(Msg::PaginationFailed {
        room: id(C),
        error: "offline".into(),
    });
    load(&mut app, C, msgs(1..=5));
    assert!(app.timeline_follows());
}

#[test]
fn a_paste_or_losing_the_room_ends_the_wait_and_its_grace_period() {
    // A paste while older messages load.
    let mut app = waiting_for_message_4();
    app.update(Msg::Paste {
        text: "x".into(),
        editor: None,
    });
    load(&mut app, C, msgs(1..=5));
    assert!(app.timeline_follows());

    // A paste during the grace period: no skip on to `!a` afterwards.
    let mut app = waiting_for_message_4();
    let grace = start_loaded_without_message_4(&mut app);
    app.update(Msg::Paste {
        text: "x".into(),
        editor: None,
    });
    assert_eq!(app.update(Msg::Timeout { id: grace }), vec![]);
    assert_eq!(open_room(&app), C);

    // `!c` removed during the grace period.
    let mut app = waiting_for_message_4();
    let grace = start_loaded_without_message_4(&mut app);
    app.update(Msg::RoomRemoved(id(C)));
    assert_eq!(app.update(Msg::Timeout { id: grace }), vec![]);
    assert_eq!(open_room(&app), "");
}

#[test]
fn tab_and_ctrl_o_stay_with_the_tables_that_do_not_fall_back_to_normal() {
    let mut app = opened_with(3);
    // A jump to go back to, were `normal` consulted.
    keys(&mut app, ":open b:ex<CR>");
    load(&mut app, B, msgs(11..=12));
    // Each key is released to the editor on its own.
    let both: Vec<Effect> = seq("<Tab><C-o>")
        .keys()
        .iter()
        .map(|&key| Effect::Ui(UiEffect::ReplayKeys(vec![key])))
        .collect();

    // The composer's insert mode: both go to the editor.
    keys(&mut app, "i");
    assert_eq!(
        no_timers(keys_in(&mut app, "<Tab><C-o>", Some(EditorMode::Insert))),
        both.clone()
    );
    // The mention menu: `<Tab>` accepts, `<C-o>` goes to the editor.
    app.update(Msg::MembersLoaded {
        room: id(B),
        members: vec![Member {
            user: UserId::new("@alice:example.org"),
            display_name: Some("Alice".into()),
        }],
    });
    app.update(Msg::MentionQuery {
        timeline: main_of(B),
        query: Some("al".into()),
    });
    assert_eq!(
        no_timers(keys_in(&mut app, "<C-o>", Some(EditorMode::Insert))),
        vec![Effect::Ui(UiEffect::ReplayKeys(
            seq("<C-o>").keys().to_vec()
        ))]
    );
    assert_eq!(
        no_timers(keys_in(&mut app, "<Tab>", Some(EditorMode::Insert))),
        vec![Effect::Ui(UiEffect::ReplaceMentionToken {
            timeline: main_of(B),
            text: "@Alice".into(),
        })]
    );
    assert_eq!((open_room(&app), app.focus()), (B, Focus::Composer));
    keys_in(&mut app, "<Esc>", Some(EditorMode::Normal));

    // A picker: `<Tab>` selects the next item, `<C-o>` edits nothing.
    keys(&mut app, "<Space><Space><Tab><C-o>");
    assert_eq!(app.picker().map(|picker| picker.selected), Some(1));
    keys(&mut app, "<Esc>");
    // The command line: `<Tab>` completes, `<C-o>` edits nothing.
    keys(&mut app, ":op<Tab><C-o>");
    assert_eq!(
        app.command_line().map(|line| line.input.text()),
        Some("open ")
    );
    keys(&mut app, "<Esc>");
    // The message view: both go to its editor.
    keys(&mut app, "<CR>");
    assert_eq!(
        no_timers(keys_in(&mut app, "<Tab><C-o>", Some(EditorMode::Normal))),
        both
    );
    assert!(app.view_open());
    assert_eq!(open_room(&app), B);
}

#[test]
fn in_the_composers_normal_mode_ctrl_o_and_tab_jump() {
    let mut app = opened_with(3);
    keys(&mut app, ":open b:ex<CR>i");
    keys_in(&mut app, "<C-o>", Some(EditorMode::Normal));
    assert_eq!((open_room(&app), app.focus()), (C, Focus::Timeline));
    keys(&mut app, "i");
    keys_in(&mut app, "<Tab>", Some(EditorMode::Visual));
    assert_eq!((open_room(&app), app.focus()), (B, Focus::Timeline));
}

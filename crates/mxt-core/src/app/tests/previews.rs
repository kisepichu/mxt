//! The timeline previews' queue (design, "Images in the timeline"): the
//! wanted items fetched at most four at a time across rooms, the answers
//! passed to the tui adapter, and stickers in the message view.

use super::timeline::{event, local, msg, opened, remote, reset, status};
use super::*;
use crate::effect::ViewContent;
use crate::timeline::{ItemRef, MediaBytes, MessageContent, TimelineDiff, TimelineEntry};

const C: &str = "!c:example.org";
const B: &str = "!b:example.org";

/// `msg(n)` with `content`.
fn with_content(n: u32, content: MessageContent) -> TimelineEntry {
    let mut entry = msg(n);
    if let TimelineEntry::Item(item) = &mut entry {
        item.content = content;
    }
    entry
}
fn image(n: u32) -> TimelineEntry {
    with_content(
        n,
        MessageContent::Image {
            name: format!("photo{n}.png"),
            size: None,
            dimensions: Some((640, 480)),
        },
    )
}
fn sticker(n: u32) -> TimelineEntry {
    with_content(
        n,
        MessageContent::Sticker {
            body: "a happy cat".into(),
            dimensions: Some((256, 256)),
        },
    )
}
/// `!c` open with images `1..=count`.
fn images(count: u32) -> App {
    let mut app = opened();
    reset(&mut app, (1..=count).map(image).collect());
    app
}
fn items(numbers: &[u32]) -> Vec<ItemRef> {
    numbers.iter().copied().map(remote).collect()
}
fn wanted(app: &mut App, numbers: &[u32]) -> Vec<Effect> {
    wanted_in(app, C, numbers)
}
fn wanted_in(app: &mut App, room: &str, numbers: &[u32]) -> Vec<Effect> {
    app.update(Msg::PreviewsWanted {
        timeline: main_of(room),
        items: items(numbers),
    })
}
fn fetch(n: u32) -> Effect {
    fetch_in(C, n)
}
fn fetch_in(room: &str, n: u32) -> Effect {
    Effect::Matrix(MatrixEffect::FetchPreview {
        room: id(room),
        item: remote(n),
    })
}
fn fetches(numbers: &[u32]) -> Vec<Effect> {
    numbers.iter().copied().map(fetch).collect()
}
fn png() -> MediaBytes {
    MediaBytes::from(b"\x89PNG".to_vec())
}
fn fetched_in(app: &mut App, room: &str, n: u32) -> Vec<Effect> {
    app.update(Msg::PreviewFetched {
        room: id(room),
        item: remote(n),
        bytes: png(),
    })
}
fn fetched(app: &mut App, n: u32) -> Vec<Effect> {
    fetched_in(app, C, n)
}
fn failed(app: &mut App, n: u32) -> Vec<Effect> {
    app.update(Msg::PreviewFailed {
        room: id(C),
        item: remote(n),
    })
}
fn shown(n: u32) -> Effect {
    Effect::Ui(UiEffect::ShowPreview {
        item: remote(n),
        bytes: png(),
    })
}

#[test]
fn four_wanted_previews_are_fetched_at_a_time_and_each_answer_starts_the_next() {
    let mut app = images(6);
    assert_eq!(
        wanted(&mut app, &[1, 2, 3, 4, 5, 6]),
        fetches(&[1, 2, 3, 4])
    );
    assert_eq!(fetched(&mut app, 1), vec![shown(1), fetch(5)]);
    // A newer list without 6: the items in flight are not fetched again,
    // and 6 never is.
    assert_eq!(wanted(&mut app, &[5, 4, 3, 2]), vec![]);
    assert_eq!(fetched(&mut app, 2), vec![shown(2)]);
    assert_eq!(
        failed(&mut app, 3),
        vec![Effect::Ui(UiEffect::PreviewFailed(remote(3)))]
    );
    // An answer that is not in flight, given twice or never asked for.
    assert_eq!(fetched(&mut app, 3), vec![]);
    assert_eq!(fetched(&mut app, 6), vec![]);
}

#[test]
fn a_newer_list_puts_its_first_items_next_in_line() {
    let mut app = images(8);
    assert_eq!(
        wanted(&mut app, &[1, 2, 3, 4, 5, 6]),
        fetches(&[1, 2, 3, 4])
    );
    assert_eq!(wanted(&mut app, &[8, 7, 6]), vec![]);
    assert_eq!(fetched(&mut app, 4), vec![shown(4), fetch(8)]);
    assert_eq!(fetched(&mut app, 3), vec![shown(3), fetch(7)]);
}

#[test]
fn answers_for_another_room_or_after_opening_another_room_are_dropped() {
    let mut app = images(2);
    assert_eq!(wanted(&mut app, &[1, 2]), fetches(&[1, 2]));
    assert_eq!(fetched_in(&mut app, "!a:example.org", 1), vec![]);
    keys(&mut app, "<C-w>h");
    keys(&mut app, "j<CR>");
    assert_eq!(app.open_room(), Some(&id("!b:example.org")));
    assert_eq!(fetched_in(&mut app, C, 1), vec![]);
    assert_eq!(fetched_in(&mut app, "!b:example.org", 2), vec![]);
}

#[test]
fn the_fetches_in_flight_count_across_rooms_and_each_old_answer_starts_the_next() {
    let mut app = images(4);
    assert_eq!(wanted(&mut app, &[1, 2, 3, 4]), fetches(&[1, 2, 3, 4]));
    keys(&mut app, "<C-w>h");
    keys(&mut app, "j<CR>");
    assert_eq!(app.open_room(), Some(&id(B)));
    app.update(Msg::Timeline {
        timeline: main_of(B),
        diffs: vec![TimelineDiff::Reset((5..=6).map(image).collect())],
    });
    // `!c`'s four fetches still run: `!b`'s wait for their slots.
    assert_eq!(wanted_in(&mut app, B, &[5, 6]), vec![]);
    assert_eq!(fetched(&mut app, 1), vec![fetch_in(B, 5)]);
    assert_eq!(failed(&mut app, 2), vec![fetch_in(B, 6)]);
    assert_eq!(fetched_in(&mut app, B, 5), vec![shown(5)]);
}

#[test]
fn a_list_for_a_timeline_that_is_not_shown_is_dropped() {
    let mut app = images(3);
    let thread = TimelineId {
        room: id(C),
        thread: Some(event(3)),
    };
    let fx = app.update(Msg::PreviewsWanted {
        timeline: thread,
        items: items(&[1]),
    });
    assert_eq!(fx, vec![]);
    let fx = app.update(Msg::PreviewsWanted {
        timeline: main_of("!a:example.org"),
        items: items(&[1]),
    });
    assert_eq!(fx, vec![]);
}

#[test]
fn a_thread_shown_in_place_of_the_timeline_fetches_none_of_its_wanted_previews() {
    let mut app = images(6);
    assert_eq!(
        wanted(&mut app, &[1, 2, 3, 4, 5, 6]),
        fetches(&[1, 2, 3, 4])
    );
    // `t` on 6: the thread's own list is reported by the next frame.
    keys(&mut app, "t");
    assert!(app.thread().is_some());
    assert_eq!(fetched(&mut app, 1), vec![shown(1)]);
}

#[test]
fn only_remote_images_and_stickers_of_the_shown_timeline_are_fetched() {
    let mut app = opened();
    let mut echo = image(2);
    if let TimelineEntry::Item(item) = &mut echo {
        item.id = local("t2");
    }
    reset(&mut app, vec![image(1), echo, msg(3), sticker(4)]);
    let fx = app.update(Msg::PreviewsWanted {
        timeline: main_of(C),
        items: vec![remote(1), local("t2"), remote(3), remote(4), remote(9)],
    });
    assert_eq!(fx, fetches(&[1, 4]));
}

#[test]
fn a_room_opened_again_after_it_was_gone_fetches_what_is_not_in_flight_once_its_list_comes() {
    let mut app = images(2);
    assert_eq!(wanted(&mut app, &[1, 2]), fetches(&[1, 2]));
    // The core drops the wanted list with the room. The tui adapter reports
    // the list again only after a frame without the room (design,
    // "Fetching").
    app.update(Msg::RoomRemoved(id(C)));
    assert_eq!(fetched(&mut app, 1), vec![]);
    app.update(Msg::RoomUpserted(room(C, 30, 0)));
    keys(&mut app, "<C-w>h");
    keys(&mut app, "gg<CR>");
    assert_eq!(app.open_room(), Some(&id(C)));
    reset(&mut app, (1..=2).map(image).collect());
    // 2 is still in flight: it is not fetched twice, and its answer passes
    // on now that its room is open again.
    assert_eq!(wanted(&mut app, &[1, 2]), fetches(&[1]));
    assert_eq!(fetched(&mut app, 2), vec![shown(2)]);
}

#[test]
fn enter_on_a_sticker_shows_its_image_in_the_message_view() {
    let mut app = opened();
    let mut echo = sticker(2);
    if let TimelineEntry::Item(item) = &mut echo {
        item.id = local("t2");
    }
    reset(&mut app, vec![sticker(1), echo]);
    assert_eq!(keys(&mut app, "<CR>"), vec![]);
    assert_eq!(status(&app), Some("Wait until the message is sent"));
    keys(&mut app, "k");
    assert_eq!(
        keys(&mut app, "<CR>"),
        vec![
            Effect::Ui(UiEffect::OpenView(ViewContent::Image {
                item: remote(1),
                name: "a happy cat".into(),
                size: None,
                dimensions: Some((256, 256)),
                reactions: Vec::new(),
            })),
            Effect::Matrix(MatrixEffect::FetchImage {
                room: id(C),
                item: remote(1),
            }),
        ]
    );
}

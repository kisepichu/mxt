//! The search highlight (design, "Search and jumps"): turned on by every
//! search, off by `:noh` and by opening another room, kept across threads,
//! and given to the message view.

use std::ops::Range;

use super::timeline::{item, msg, opened, reacted, remote, reset, status};
use super::*;
use crate::effect::ViewContent;
use crate::search::Needle;

fn shown(app: &App) -> Option<Needle> {
    app.search_highlight().cloned()
}

fn hello() -> Option<Needle> {
    Some(Needle::new("hello"))
}

/// The message view of `text` with the search's `matches` on it, and no
/// reactions.
fn message_view(text: &str, matches: Vec<Range<usize>>) -> Effect {
    Effect::Ui(UiEffect::OpenView(ViewContent::Text {
        text: text.into(),
        matches,
        reactions: Vec::new(),
    }))
}

#[test]
fn every_search_shows_the_highlight_until_noh() {
    let mut app = opened();
    reset(
        &mut app,
        vec![item(remote(1), "Hello world"), item(remote(2), "say hello")],
    );
    assert_eq!(shown(&app), None);
    // Without regard to case, as the search matches.
    keys(&mut app, "/HELLO<CR>");
    assert_eq!(shown(&app), hello());
    keys(&mut app, "n");
    assert_eq!(shown(&app), hello());
    keys(&mut app, ":noh<CR>");
    assert_eq!(shown(&app), None);
    // The next search shows it again.
    keys(&mut app, "N");
    assert_eq!(shown(&app), hello());
    keys(&mut app, ":nohlsearch<CR>");
    assert_eq!(shown(&app), None);
    // Also one that finds nothing, as in nvim.
    keys(&mut app, "/zzz<CR>");
    assert_eq!(status(&app), Some("Pattern not found: zzz"));
    assert_eq!(shown(&app), Some(Needle::new("zzz")));
    // `:noh` takes no argument and then hides nothing.
    keys(&mut app, ":noh x<CR>");
    assert_eq!(status(&app), Some(":noh takes no argument"));
    assert_eq!(shown(&app), Some(Needle::new("zzz")));
    // A search in the room list shows it too: the panes share the pattern.
    keys(&mut app, ":noh<CR><C-w>h/example<CR>");
    assert_eq!(app.focus(), Focus::RoomList);
    assert_eq!(shown(&app), Some(Needle::new("example")));
}

#[test]
fn opening_another_room_hides_the_highlight_and_a_thread_keeps_it() {
    let mut app = opened();
    reset(&mut app, (1..4).map(msg).collect());
    keys(&mut app, "/message 2<CR>");
    let needle = Some(Needle::new("message 2"));
    // A thread is in the same room.
    keys(&mut app, "t");
    assert!(app.thread().is_some());
    assert_eq!(shown(&app), needle);
    keys(&mut app, "q");
    assert!(app.thread().is_none());
    assert_eq!(shown(&app), needle);
    // `<CR>` on the open room only focuses its timeline.
    keys(&mut app, "<C-w>hgg<CR>");
    assert_eq!(app.open_room(), Some(&id("!c:example.org")));
    assert_eq!(shown(&app), needle);
    keys(&mut app, "<C-w>hj<CR>");
    assert_eq!(app.open_room(), Some(&id("!b:example.org")));
    assert_eq!(shown(&app), None);
    // So does the open room's removal.
    keys(&mut app, "/message<CR>");
    assert_eq!(shown(&app), Some(Needle::new("message")));
    app.update(Msg::RoomRemoved(id("!b:example.org")));
    assert_eq!(app.open_room(), None);
    assert_eq!(shown(&app), None);
}

#[test]
fn the_message_view_gets_the_matches_while_the_highlight_is_shown() {
    let mut app = opened();
    reset(&mut app, vec![item(remote(1), "Hello, hello")]);
    keys(&mut app, "/hello<CR>");
    assert_eq!(
        keys(&mut app, "<CR>"),
        vec![message_view("Hello, hello", vec![0..5, 7..12])]
    );
    keys_in(&mut app, "q", Some(EditorMode::Normal));
    keys(&mut app, ":noh<CR>");
    assert_eq!(
        keys(&mut app, "<CR>"),
        vec![message_view("Hello, hello", Vec::new())]
    );
}

#[test]
fn the_message_views_matches_are_in_its_text_not_in_the_reaction_lines() {
    let mut app = opened();
    let thumbs = group("👍", &["@hello:example.org"]);
    reset(
        &mut app,
        vec![reacted(item(remote(1), "hello, say hello"), vec![thumbs])],
    );
    keys(&mut app, "/hello<CR>");
    assert_eq!(
        keys(&mut app, "<CR>"),
        vec![Effect::Ui(UiEffect::OpenView(ViewContent::Text {
            text: "hello, say hello".into(),
            matches: vec![0..5, 11..16],
            reactions: vec!["👍 @hello:example.org".into()],
        }))]
    );
}

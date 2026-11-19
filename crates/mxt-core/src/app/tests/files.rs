use super::timeline::{msg, opened, remote, reset, status};
use super::*;
use crate::effect::ViewContent;
use crate::rich::{Block, Inline, MentionTarget, RichText, Style};
use crate::settings::ImagePreview;
use crate::timeline::{ItemRef, MediaBytes, MessageContent, TimelineEntry};

const C: &str = "!c:example.org";

fn ui(effect: UiEffect) -> Effect {
    Effect::Ui(effect)
}
fn matrix(effect: MatrixEffect) -> Effect {
    Effect::Matrix(effect)
}
/// Message `n` with `content`.
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
            name: "cat.png".into(),
            size: Some(2048),
            dimensions: Some((640, 480)),
        },
    )
}
fn file(n: u32) -> TimelineEntry {
    with_content(
        n,
        MessageContent::File {
            name: "notes.pdf".into(),
            size: Some(1024),
            mime: Some("application/pdf".into()),
        },
    )
}
/// A text message of `blocks`.
fn rich(n: u32, blocks: Vec<Block>) -> TimelineEntry {
    let rich = RichText { blocks };
    let body = rich.to_plain();
    with_content(n, MessageContent::Text { body, rich })
}
fn text(text: &str) -> Inline {
    Inline::Text {
        text: text.into(),
        style: Style::default(),
    }
}
fn link(text: &str, url: &str) -> Inline {
    Inline::Link {
        text: text.into(),
        url: url.into(),
        style: Style::default(),
    }
}
fn gx(app: &mut App) -> Vec<Effect> {
    no_timers(keys(app, "gx"))
}
fn download(item: ItemRef) -> Effect {
    matrix(MatrixEffect::Download { room: id(C), item })
}
fn open_url(url: &str) -> Effect {
    ui(UiEffect::OpenUrl(url.into()))
}
fn image_view(n: u32) -> Effect {
    ui(UiEffect::OpenView(ViewContent::Image {
        item: remote(n),
        name: "cat.png".into(),
        size: Some(2048),
        dimensions: Some((640, 480)),
    }))
}
fn fetch(n: u32) -> Effect {
    matrix(MatrixEffect::FetchImage {
        room: id(C),
        item: remote(n),
    })
}
fn png() -> MediaBytes {
    MediaBytes::from(b"\x89PNG".to_vec())
}
fn fetched(app: &mut App, n: u32) -> Vec<Effect> {
    app.update(Msg::ImageFetched {
        item: remote(n),
        bytes: png(),
    })
}
fn unavailable(app: &mut App, n: u32, reason: &str) -> Vec<Effect> {
    app.update(Msg::ImageUnavailable {
        item: remote(n),
        reason: reason.into(),
    })
}

#[test]
fn gx_on_an_attachment_downloads_it_and_opens_the_saved_file() {
    let mut app = opened();
    reset(&mut app, vec![image(1), file(2)]);
    assert_eq!(gx(&mut app), vec![download(remote(2))]);
    keys(&mut app, "k");
    assert_eq!(gx(&mut app), vec![download(remote(1))]);
    let path = "/downloads/cat (1).png";
    assert_eq!(
        app.update(Msg::Downloaded { path: path.into() }),
        vec![ui(UiEffect::OpenPath(path.into()))]
    );
    assert_eq!(status(&app), Some("Saved /downloads/cat (1).png"));
}

#[test]
fn gx_opens_the_only_link_of_a_message() {
    let mut app = opened();
    let docs = "https://example.org/docs";
    reset(
        &mut app,
        vec![rich(
            1,
            vec![Block::Paragraph(vec![text("see "), link("the docs", docs)])],
        )],
    );
    assert_eq!(gx(&mut app), vec![open_url(docs)]);
    assert!(app.picker().is_none());
}

#[test]
fn gx_on_several_links_picks_one_of_their_urls() {
    let docs = "https://example.org/docs";
    let blog = "https://example.org/blog";
    let code = "https://example.com/code";
    let mut app = opened();
    reset(
        &mut app,
        vec![rich(
            1,
            vec![
                Block::Paragraph(vec![link("docs", docs), text(" and "), link("again", docs)]),
                Block::Quote(vec![Block::Paragraph(vec![link("blog", blog)])]),
                Block::List {
                    ordered: false,
                    start: 1,
                    items: vec![vec![Block::Paragraph(vec![link("code", code)])]],
                },
                Block::Code {
                    language: None,
                    text: "https://example.org/not-a-link".into(),
                },
            ],
        )],
    );
    assert_eq!(gx(&mut app), vec![]);
    let picker = app.picker().unwrap();
    assert_eq!(picker.kind, PickerKind::Links);
    assert_eq!(app.mode_label(None), "PICKER");
    // In order, each URL once.
    let urls: Vec<(&str, &str)> = picker
        .items
        .iter()
        .map(|item| (item.label.as_str(), item.key.as_str()))
        .collect();
    assert_eq!(urls, [(docs, docs), (blog, blog), (code, code)]);
    keys(&mut app, "blog");
    assert_eq!(app.picker().unwrap().items.len(), 1);
    assert_eq!(no_timers(keys(&mut app, "<CR>")), vec![open_url(blog)]);
    assert!(app.picker().is_none());
}

#[test]
fn gx_without_a_link_or_an_attachment_has_nothing_to_open() {
    let mention = Inline::Mention {
        text: "@Alice".into(),
        target: MentionTarget::User(UserId::new("@alice:example.org")),
    };
    let mut app = opened();
    reset(
        &mut app,
        vec![
            with_content(1, MessageContent::Redacted),
            rich(2, vec![Block::Paragraph(vec![text("hi "), mention])]),
        ],
    );
    assert_eq!(gx(&mut app), vec![]);
    assert_eq!(status(&app), Some("Nothing to open"));
    keys(&mut app, "k");
    assert_eq!(gx(&mut app), vec![]);
    assert_eq!(status(&app), Some("Nothing to open"));
    assert!(app.picker().is_none());
}

#[test]
fn upload_sends_the_file_to_the_open_room() {
    let mut app = app();
    assert_eq!(
        no_timers(keys(&mut app, ":upload /tmp/a b.png<CR>")),
        vec![]
    );
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    assert_eq!(status(&app), Some("No room is open"));
    keys(&mut app, "<CR>");
    assert_eq!(
        no_timers(keys(&mut app, ":upload /tmp/a b.png<CR>")),
        vec![matrix(MatrixEffect::Upload {
            room: id(C),
            path: "/tmp/a b.png".into(),
        })]
    );
}

#[test]
fn the_image_view_fetches_the_image_and_shows_the_one_for_its_item() {
    let mut app = opened();
    reset(&mut app, vec![image(1), image(2)]);
    assert_eq!(keys(&mut app, "<CR>"), vec![image_view(2), fetch(2)]);
    assert!(app.view_open());
    assert_eq!(fetched(&mut app, 1), vec![]);
    assert_eq!(
        fetched(&mut app, 2),
        vec![ui(UiEffect::ShowImage {
            item: remote(2),
            bytes: png(),
        })]
    );
    keys_in(&mut app, "qk", Some(EditorMode::Normal));
    assert_eq!(keys(&mut app, "<CR>"), vec![image_view(1), fetch(1)]);
    assert_eq!(unavailable(&mut app, 1, "too large to preview"), vec![]);
    assert_eq!(status(&app), Some("No image preview: too large to preview"));
}

#[test]
fn with_image_previews_off_the_view_shows_the_image_details_only() {
    let off = Settings {
        image_preview: ImagePreview::Off,
        ..Settings::default()
    };
    let mut app = app_in(off, vec![]);
    keys(&mut app, "<CR>");
    reset(&mut app, vec![image(1)]);
    assert_eq!(keys(&mut app, "<CR>"), vec![image_view(1)]);
    assert!(app.view_open());
}

#[test]
fn an_image_that_arrives_after_its_view_closed_is_dropped() {
    let mut app = opened();
    reset(&mut app, vec![image(1)]);
    keys(&mut app, "<CR>");
    assert_eq!(
        keys_in(&mut app, "q", Some(EditorMode::Normal)),
        vec![ui(UiEffect::CloseView)]
    );
    assert_eq!(fetched(&mut app, 1), vec![]);
    assert_eq!(unavailable(&mut app, 1, "timed out"), vec![]);
    assert_eq!(app.status(), None);
}

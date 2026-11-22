use std::io::Cursor;
use std::time::Duration;

use image::{ImageFormat, Rgb, RgbImage};
use mxt_core::ids::{EventId, RoomId, TimelineId, Timestamp, UserId};
use mxt_core::mention::Member;
use mxt_core::timeline::{MessageContent, TimelineDiff, TimelineEntry, TimelineItem};
use ratatui::buffer::Cell;
use ratatui::style::Color;
use ratatui_image::protocol::kitty::Kitty;

use super::*;
use crate::view::RoomListLayout;
use crate::view::tests::{Screen, app, message, press, reset, upsert};

const ROOM: &str = "!a:example.org";
/// The cell size of tmux's sixel in these tests, and of half blocks.
const CELL: FontSize = FontSize::new(10, 20);

fn item(n: u32) -> ItemRef {
    ItemRef::Remote(EventId::new(format!("${n}:example.org")))
}

/// The image message with the event `$<n>` and the image's `dimensions`.
fn image(n: u32, dimensions: (u32, u32)) -> TimelineEntry {
    TimelineEntry::Item(TimelineItem {
        id: item(n),
        sender: UserId::new("@alice:example.org"),
        sender_name: "Alice".to_owned(),
        timestamp: Timestamp::from_millis(0),
        content: MessageContent::Image {
            name: format!("photo{n}.png"),
            size: None,
            dimensions: Some(dimensions),
        },
        reply: None,
        reactions: Vec::new(),
        edited: false,
        send_state: None,
        thread_summary: None,
    })
}

/// `width` by `height` pixels, bright at the top and dark at the bottom,
/// so that every half block has a brighter upper half: `▀`.
fn gradient(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |_, y| {
        let level = u8::try_from(255 - 255 * y / height).unwrap();
        Rgb([level, level, level])
    })
}

/// A PNG of `gradient(width, height)`.
fn png(width: u32, height: u32) -> MediaBytes {
    let mut bytes = Vec::new();
    gradient(width, height)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes.into()
}

fn tmux() -> PreviewEncoder {
    PreviewEncoder::TmuxSixel(CELL)
}

/// Half blocks, which need no picker: a picker reads `TERM` and
/// `TERM_PROGRAM`, and inside tmux turns on the pane's `allow-passthrough`.
fn halfblocks() -> PreviewEncoder {
    PreviewEncoder::Halfblocks
}

fn previews(rows: u16, encoder: PreviewEncoder, limits: Limits) -> Previews {
    let rows = ImageRows::new(rows.into()).unwrap();
    Previews::new(rows, encoder, limits, Box::new(|| {})).unwrap()
}

/// A screen of `height` rows with previews, which `open` gives the room.
fn screen(height: u16, previews: Previews) -> Screen {
    let mut screen = Screen::of(80, height, RoomListLayout::default());
    screen.previews = Some(previews);
    screen
}

/// The room `!a` open with `entries` without the room list, so that the
/// entries' text starts in column 2, and one frame drawn, whose report is
/// dropped: the cursor follows the newest item. The entries are on rows 1
/// to `height - 4`, below the room's name and above the composer.
fn open(screen: &mut Screen, entries: Vec<TimelineEntry>) -> App {
    let mut app = app();
    press(&mut app, "<CR>");
    press(&mut app, "<Space>e");
    app.update(Msg::Timeline {
        timeline: TimelineId::main(RoomId::new(ROOM)),
        diffs: vec![TimelineDiff::Reset(entries)],
    });
    screen.draw(&app, None);
    previews_of(screen).take_report();
    app
}

fn previews_of(screen: &mut Screen) -> &mut Previews {
    screen.previews.as_mut().unwrap()
}

/// The entries with a row on the pane and the wanted list a frame reports,
/// if it reports one.
type Frame = (Option<(usize, usize)>, Option<Vec<ItemRef>>);

fn frame(app: &App, screen: &mut Screen) -> Frame {
    let visible = screen.draw(app, None).timeline;
    let wanted = previews_of(screen)
        .take_report()
        .map(|report| match report {
            Msg::PreviewsWanted { timeline, items } => {
                assert_eq!(timeline, TimelineId::main(RoomId::new(ROOM)));
                items
            }
            other => panic!("not a list: {other:?}"),
        });
    (visible, wanted)
}

/// The wanted list a frame reports.
fn wanted(app: &App, screen: &mut Screen) -> Option<Vec<ItemRef>> {
    frame(app, screen).1
}

/// Waits for the worker to finish every job, re-encodings too.
fn settle(previews: &mut Previews) {
    loop {
        while previews.working.is_some() {
            let done = previews
                .done
                .recv_timeout(Duration::from_secs(30))
                .expect("the worker answers");
            previews.accept(done);
        }
        previews.settle_cache();
        if previews.working.is_none() {
            return;
        }
    }
}

/// The fetched image `bytes` of each of `items`, decoded by the worker.
fn show(screen: &mut Screen, items: &[u32], bytes: &MediaBytes) {
    let previews = previews_of(screen);
    for &n in items {
        previews.show(item(n), bytes.clone());
    }
    settle(previews);
}

fn kept(previews: &Previews) -> Vec<ItemRef> {
    let mut kept: Vec<ItemRef> = previews.kept.iter().map(|(item, _)| item.clone()).collect();
    kept.sort_by_key(|item| format!("{item:?}"));
    kept
}

fn items(numbers: &[u32]) -> Vec<ItemRef> {
    let mut items: Vec<ItemRef> = numbers.iter().copied().map(item).collect();
    items.sort_by_key(|item| format!("{item:?}"));
    items
}

/// Whether a cell holds a sixel sequence: drawn by ratatui-image into the
/// first cell of the image's area.
fn sixel(cell: &Cell) -> bool {
    cell.symbol().contains("\x1bP")
}

fn any_sixel(screen: &Screen) -> bool {
    screen.terminal.backend().buffer().content.iter().any(sixel)
}

#[test]
fn image_rows_are_from_1_to_50_and_8_by_default() {
    assert_eq!(ImageRows::new(0), None);
    assert_eq!(ImageRows::new(1).map(ImageRows::get), Some(1));
    assert_eq!(ImageRows::new(50).map(ImageRows::get), Some(50));
    assert_eq!(ImageRows::new(51), None);
    assert_eq!(ImageRows::new(-8), None);
    assert_eq!(ImageRows::default().get(), 8);
}

#[test]
fn a_preview_reserves_the_fit_of_the_image_into_its_box_in_whole_cells() {
    let rows = ImageRows::default();
    // A box of 600 by 160 pixels, into which 4000 by 3000 fits as 213 by
    // 160 pixels: 22 columns, 8 rows.
    assert_eq!(
        reserve(Some((4000, 3000)), 60, rows, CELL),
        Size::new(22, 8)
    );
    // Never larger than the image.
    assert_eq!(reserve(Some((100, 50)), 60, rows, CELL), Size::new(10, 3));
    // A wide image is limited by the width.
    assert_eq!(reserve(Some((3000, 100)), 60, rows, CELL), Size::new(60, 1));
    // Without dimensions, or with none that make sense, the box.
    assert_eq!(reserve(None, 60, rows, CELL), Size::new(60, 8));
    assert_eq!(reserve(Some((0, 300)), 60, rows, CELL), Size::new(60, 8));
}

#[test]
fn the_worker_scales_an_image_to_its_reserved_size_and_encodes_it() {
    let encoder = halfblocks();
    // 64 by 32 pixels fit a box of 60 by 8 cells as 7 by 2 cells.
    let size = reserve(Some((64, 32)), 60, ImageRows::default(), CELL);
    assert_eq!(size, Size::new(7, 2));
    let job = Job {
        item: item(1),
        source: Source::Bytes(png(64, 32)),
        target: Target { size, scale: true },
    };
    let Done::Ready(ready, preview) = prepare(job, &encoder) else {
        panic!("the preview failed");
    };
    assert_eq!(ready, item(1));
    assert!(matches!(
        preview.encoded,
        Encoded::Sliced(SlicedProtocol::Halfblocks(_))
    ));
    assert_eq!(preview.encoded.size(), size);
    // Scaled up to fill 70 by 40 pixels in proportion.
    let scaled = (preview.scaled.width(), preview.scaled.height());
    assert_eq!(scaled, (70, 35));

    // Without dimensions only the image's own size, not the box.
    let job = Job {
        item: item(2),
        source: Source::Bytes(png(64, 32)),
        target: Target {
            size: Size::new(60, 8),
            scale: false,
        },
    };
    let Done::Ready(_, preview) = prepare(job, &encoder) else {
        panic!("the preview failed");
    };
    assert_eq!(preview.encoded.size(), Size::new(7, 2));

    let mut truncated = png(64, 32).as_bytes().to_vec();
    truncated.truncate(truncated.len() / 2);
    let job = Job {
        item: item(3),
        source: Source::Bytes(truncated.into()),
        target: Target { size, scale: true },
    };
    assert!(matches!(prepare(job, &encoder), Done::Failed(failed) if failed == item(3)));
}

#[test]
fn a_panic_while_making_a_preview_fails_that_preview_alone() {
    let done = answer(item(1), || panic!("a decoder's bug"));
    assert!(matches!(done, Done::Failed(failed) if failed == item(1)));
}

/// Thirty messages of two rows, with images of four rows at 5, 12 and 25.
fn thirty() -> Vec<TimelineEntry> {
    (0..30)
        .map(|n| match n {
            5 | 12 | 25 => image(n, (100, 50)),
            _ => message(n),
        })
        .collect()
}

/// `thirty` open on a pane of 14 rows.
fn thirty_messages() -> (App, Screen) {
    let mut screen = screen(18, previews(2, tmux(), Limits::DEFAULT));
    let app = open(&mut screen, thirty());
    (app, screen)
}

#[test]
fn the_wanted_list_holds_the_images_on_the_pane_then_those_near_it() {
    let (mut app, mut screen) = thirty_messages();
    // Up to the message at 10, which comes in at the pane's top: the pane
    // shows 10 to 15, and as many entries again each way are near it.
    for _ in 0..19 {
        press(&mut app, "k");
    }
    let targets = vec![item(12), item(5)];
    assert_eq!(frame(&app, &mut screen), (Some((10, 15)), Some(targets)));
    // A frame with the same pane reports nothing.
    assert_eq!(frame(&app, &mut screen), (Some((10, 15)), None));
    // A kept preview and a failed one are no longer wanted.
    show(&mut screen, &[12], &png(100, 50));
    assert_eq!(wanted(&app, &mut screen), Some(vec![item(5)]));
    previews_of(&mut screen).fail(item(5));
    assert_eq!(wanted(&app, &mut screen), Some(vec![]));
    assert_eq!(wanted(&app, &mut screen), None);
}

#[test]
fn a_failed_preview_leaves_its_line_alone_until_another_timeline_is_shown() {
    let (mut app, mut screen) = thirty_messages();
    // The pane shows 24 to 29, the image at 25 with its rows.
    assert_eq!(frame(&app, &mut screen), (Some((24, 29)), None));
    previews_of(&mut screen).fail(item(25));
    // Its two rows are gone: 23 has a row on the pane now.
    assert_eq!(frame(&app, &mut screen), (Some((23, 29)), Some(vec![])));

    // To `!b` and back.
    press(&mut app, "<C-w>hj<CR>");
    app.update(reset("!b:example.org", 1..4));
    screen.draw(&app, None);
    press(&mut app, "<C-w>hk<CR>");
    app.update(Msg::Timeline {
        timeline: TimelineId::main(RoomId::new(ROOM)),
        diffs: vec![TimelineDiff::Reset(thirty())],
    });
    let wanted = Some(vec![item(25)]);
    assert_eq!(frame(&app, &mut screen), (Some((24, 29)), wanted));
}

#[test]
fn a_room_back_after_it_was_gone_reports_its_wanted_list_again() {
    let (mut app, mut screen) = thirty_messages();
    // `open` reported 25, which the core keeps as its queue.
    assert_eq!(frame(&app, &mut screen), (Some((24, 29)), None));
    // The room leaves the list, and the core drops its queue with it.
    app.update(Msg::RoomRemoved(RoomId::new(ROOM)));
    assert_eq!(frame(&app, &mut screen), (None, None));

    // Back with the same entries: the same list, which the core no longer
    // has.
    app.update(upsert(ROOM, 20));
    press(&mut app, "<C-w>hk<CR>");
    assert_eq!(app.open_room(), Some(&RoomId::new(ROOM)));
    app.update(Msg::Timeline {
        timeline: TimelineId::main(RoomId::new(ROOM)),
        diffs: vec![TimelineDiff::Reset(thirty())],
    });
    let wanted = Some(vec![item(25)]);
    assert_eq!(frame(&app, &mut screen), (Some((24, 29)), wanted));
}

/// Twenty images of four rows, a pane of 8 rows with the two at 10 and 11,
/// and the list of the frame that shows them.
fn twenty_images(limits: Limits) -> (App, Screen, Option<Vec<ItemRef>>) {
    let entries = (0..20).map(|n| image(n, (100, 50))).collect();
    let mut screen = screen(12, previews(2, tmux(), limits));
    let mut app = open(&mut screen, entries);
    for _ in 0..9 {
        press(&mut app, "k");
    }
    let (visible, wanted) = frame(&app, &mut screen);
    assert_eq!(visible, Some((10, 11)));
    (app, screen, wanted)
}

#[test]
fn the_targets_stop_at_the_cache_s_count_and_stay_kept_while_idle() {
    let (app, mut screen, wanted_first) = twenty_images(Limits {
        count: 4,
        ..Limits::DEFAULT
    });
    // The two on the pane, then the nearest, below first.
    assert_eq!(
        wanted_first,
        Some(vec![item(10), item(11), item(12), item(9)])
    );
    show(&mut screen, &[10, 11, 12, 9], &png(100, 50));
    assert_eq!(wanted(&app, &mut screen), Some(vec![]));
    for _ in 0..3 {
        assert_eq!(wanted(&app, &mut screen), None);
    }
    let previews = previews_of(&mut screen);
    assert_eq!(kept(previews), items(&[9, 10, 11, 12]));
    assert!(previews.working.is_none() && previews.pending.is_empty());
}

#[test]
fn a_pane_holding_more_previews_than_the_cache_keeps_them_all() {
    let (app, mut screen, wanted_first) = twenty_images(Limits {
        count: 1,
        ..Limits::DEFAULT
    });
    assert_eq!(wanted_first, Some(vec![item(10), item(11)]));
    show(&mut screen, &[10, 11], &png(100, 50));
    assert_eq!(wanted(&app, &mut screen), Some(vec![]));
    assert_eq!(wanted(&app, &mut screen), None);
    assert_eq!(kept(previews_of(&mut screen)), items(&[10, 11]));
}

#[test]
fn the_targets_stop_at_the_cache_s_bytes() {
    // An image of 8 by 2 cells is estimated at 8 bytes per pixel.
    let estimate = estimate(Size::new(8, 2), CELL);
    assert_eq!(estimate, 80 * 40 * 8);
    let limits = Limits {
        bytes: 3 * estimate,
        ..Limits::DEFAULT
    };
    let (_, _, wanted_first) = twenty_images(limits);
    assert_eq!(wanted_first, Some(vec![item(10), item(11), item(12)]));
}

#[test]
fn a_scroll_lets_the_cache_drop_previews_that_are_no_targets_past_its_limit() {
    let (mut app, mut screen, _) = twenty_images(Limits {
        count: 4,
        ..Limits::DEFAULT
    });
    show(&mut screen, &[10, 11, 12, 9], &png(100, 50));
    screen.draw(&app, None);
    // Down to 14: the pane shows 13 and 14, and none of the four kept is a
    // target but 12, which stays.
    for _ in 0..4 {
        press(&mut app, "j");
    }
    let (visible, wanted) = frame(&app, &mut screen);
    assert_eq!(visible, Some((13, 14)));
    assert_eq!(wanted, Some(vec![item(13), item(14), item(15)]));
    assert_eq!(kept(previews_of(&mut screen)), items(&[9, 10, 11, 12]));
    show(&mut screen, &[13, 14, 15], &png(100, 50));
    assert_eq!(kept(previews_of(&mut screen)), items(&[12, 13, 14, 15]));
}

#[test]
fn a_job_whose_item_scrolled_away_is_dropped_before_it_is_decoded() {
    let (mut app, mut screen, _) = twenty_images(Limits {
        count: 4,
        ..Limits::DEFAULT
    });
    // 10 goes to the worker, 11 waits for it.
    let previews = previews_of(&mut screen);
    previews.show(item(10), png(100, 50));
    previews.show(item(11), png(100, 50));
    assert_eq!(previews.working, Some(item(10)));
    assert_eq!(previews.pending.len(), 1);
    // Down to 14: the pane shows 13 and 14, and neither 10 nor 11 is a
    // target.
    for _ in 0..4 {
        press(&mut app, "j");
    }
    let (visible, wanted) = frame(&app, &mut screen);
    assert_eq!(visible, Some((13, 14)));
    assert_eq!(wanted, Some(vec![item(13), item(14), item(15), item(12)]));
    let previews = previews_of(&mut screen);
    assert!(previews.pending.is_empty());
    // 10 was decoded already; 11 never is.
    settle(previews);
    assert_eq!(kept(previews), items(&[10]));
    // Back up: 11 is wanted again.
    for _ in 0..4 {
        press(&mut app, "k");
    }
    let wanted = Some(vec![item(11), item(12), item(9)]);
    assert_eq!(frame(&app, &mut screen), (Some((10, 11)), wanted));
}

/// A preview of one cell, encoded for tmux.
fn tiny() -> Preview {
    let target = Target {
        size: Size::new(1, 1),
        scale: false,
    };
    let source = Source::Scaled(DynamicImage::new_rgba8(10, 20));
    make(source, target, &tmux()).unwrap()
}

#[test]
fn past_64_previews_the_cache_drops_the_least_recently_drawn_that_is_no_target() {
    let mut previews = previews(8, tmux(), Limits::DEFAULT);
    let target = tiny().target;
    previews.targets = vec![(item(0), target)];
    for n in 0..64 {
        previews.accept(Done::Ready(item(n), tiny()));
    }
    previews.settle_cache();
    assert_eq!(previews.kept.len(), 64);
    // 1 is drawn, so 2 is the least recently drawn that is no target.
    let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 10));
    let drawn = item(1);
    let placed = Placed {
        item: &drawn,
        top: 0,
        target,
    };
    let area = buffer.area;
    previews.draw(&mut buffer, area, [placed], &[]);
    previews.accept(Done::Ready(item(64), tiny()));
    previews.settle_cache();
    assert_eq!(previews.kept.len(), 64);
    let kept = kept(&previews);
    assert!(kept.contains(&item(0)) && kept.contains(&item(1)));
    assert!(!kept.contains(&item(2)), "{kept:?}");
}

#[test]
fn past_64_mib_the_cache_drops_the_least_recently_drawn_that_is_no_target() {
    let mut previews = previews(8, tmux(), Limits::DEFAULT);
    previews.targets = vec![(item(0), tiny().target)];
    // About 16 MB each: four fit in 64 MiB, five do not.
    let large = || {
        let Preview {
            encoded,
            target,
            cell,
            ..
        } = tiny();
        Preview::new(DynamicImage::new_rgba8(2000, 2000), encoded, target, cell)
    };
    for n in 0..4 {
        previews.accept(Done::Ready(item(n), large()));
    }
    previews.settle_cache();
    assert_eq!(previews.kept.len(), 4);
    previews.accept(Done::Ready(item(4), large()));
    previews.settle_cache();
    assert_eq!(kept(&previews), items(&[0, 2, 3, 4]));
}

/// A preview of 10 by 4 cells, 100 by 80 pixels, encoded for kitty.
fn kitty() -> Preview {
    let target = Target {
        size: Size::new(10, 4),
        scale: false,
    };
    let image = DynamicImage::new_rgba8(100, 80);
    let kitty = Kitty::new(image.clone(), target.size, 1, false, false).unwrap();
    let encoded = Encoded::Sliced(SlicedProtocol::Kitty(kitty));
    Preview::new(image, encoded, target, CELL)
}

#[test]
fn past_its_bytes_the_cache_counts_what_a_kitty_preview_holds() {
    // 100 by 80 pixels are estimated at 64000 bytes, which the targets
    // count a kept kitty preview as. It holds more: 32000 bytes of RGBA
    // and kitty's transmission, their base64.
    let estimate = estimate(Size::new(10, 4), CELL);
    assert_eq!(estimate, 64_000);
    assert_eq!(kitty().counted(), estimate);
    assert_eq!(kitty().bytes, 32_000 + 42_668);
    let limits = Limits {
        bytes: 3 * estimate,
        ..Limits::DEFAULT
    };
    let mut previews = previews(8, tmux(), limits);
    for n in 0..3 {
        previews.accept(Done::Ready(item(n), kitty()));
    }
    previews.settle_cache();
    // Three fit as estimated, two as held.
    assert_eq!(kept(&previews), items(&[1, 2]));
}

/// The cells of row `y` from column 2 for `width` columns.
fn row_cells(screen: &Screen, y: u16, width: u16) -> Vec<Cell> {
    (2..2 + width).map(|x| screen.cell(x, y).clone()).collect()
}

/// Whether `cells` are half blocks drawn from an image.
fn half_blocks(cells: &[Cell]) -> bool {
    cells
        .iter()
        .all(|cell| cell.symbol() == "▀" && matches!(cell.bg, Color::Rgb(..)))
}

#[test]
fn half_blocks_fill_the_reserved_rows_below_the_image_s_line() {
    let mut screen = screen(12, previews(4, halfblocks(), Limits::DEFAULT));
    // 100 by 80 pixels take 10 by 4 cells. The item's six rows end the
    // pane: its line on screen row 4, the preview on 5 to 8.
    let app = open(&mut screen, vec![message(1), image(2, (100, 80))]);
    let line = (2..30)
        .map(|x| screen.cell(x, 4).symbol())
        .collect::<String>();
    assert_eq!(line.trim_end(), "[image: photo2.png 100x80]");
    assert!(
        !row_cells(&screen, 5, 10)
            .iter()
            .any(|cell| cell.symbol() == "▀")
    );
    show(&mut screen, &[2], &png(100, 80));
    screen.draw(&app, None);
    for y in 5..9 {
        assert!(half_blocks(&row_cells(&screen, y, 10)), "row {y}");
        assert_eq!(screen.cell(12, y).symbol(), " ", "row {y}");
    }
    assert!(!screen.again);
}

#[test]
fn half_blocks_fill_their_box_whatever_cell_size_their_pixels_are_in() {
    // 4000 by 3000 pixels reserve 22 by 8 cells in 60 columns. Fitted again
    // in the cell size a picker measured, they came out 22 by 7 at 10 by
    // 23 and at 7 by 17 pixels, and 20 by 8 at 10 by 18.
    let size = Size::new(22, 8);
    for (width, height) in [(10, 23), (7, 17), (10, 18)] {
        let padded = DynamicImage::ImageRgb8(gradient(22 * width, 8 * height));
        let Ok(Encoded::Sliced(sliced)) = encode(padded, size, &halfblocks()) else {
            panic!("not encoded as sliced half blocks");
        };
        assert_eq!(sliced.size(), size, "{width}x{height}");
        let mut buffer = Buffer::empty(Rect::new(0, 0, 30, 10));
        SlicedImage::new(&sliced, SignedPosition { x: 0, y: 0 }).render(buffer.area, &mut buffer);
        for y in 0..8 {
            let row: Vec<Cell> = (0..22).map(|x| buffer[(x, y)].clone()).collect();
            assert!(half_blocks(&row), "{width}x{height}, row {y}");
            assert_eq!(buffer[(22, y)].symbol(), " ", "{width}x{height}, row {y}");
        }
        assert_eq!(buffer[(0, 8)].symbol(), " ", "{width}x{height}");
    }
}

#[test]
fn a_preview_cut_at_the_pane_s_top_shows_its_visible_rows() {
    let mut screen = screen(12, previews(4, halfblocks(), Limits::DEFAULT));
    // The image's six rows start four rows above the pane, so the first
    // two rows of its preview are above it, the last two on screen rows 1
    // and 2.
    let entries = vec![image(0, (100, 80)), message(1), message(2), message(3)];
    let app = open(&mut screen, entries);
    show(&mut screen, &[0], &png(100, 80));
    screen.draw(&app, None);
    // The preview drawn whole elsewhere: its rows 2 and 3 are on the pane.
    let previews = previews_of(&mut screen);
    let Some(Preview {
        encoded: Encoded::Sliced(sliced),
        ..
    }) = previews.preview_of(&item(0))
    else {
        panic!("not kept");
    };
    let mut whole = Buffer::empty(Rect::new(0, 0, 10, 4));
    SlicedImage::new(sliced, SignedPosition { x: 0, y: 0 }).render(whole.area, &mut whole);
    for (y, row) in [(1, 2), (2, 3)] {
        let expected: Vec<Cell> = (0..10).map(|x| whole[(x, row)].clone()).collect();
        assert_eq!(row_cells(&screen, y, 10), expected, "row {y}");
    }
    // The message below starts on the next row.
    assert_ne!(screen.cell(2, 3).symbol(), "▀");
}

#[test]
fn tmux_s_sixel_leaves_a_preview_cut_at_the_pane_s_top_blank() {
    let mut screen = screen(12, previews(4, tmux(), Limits::DEFAULT));
    let entries = vec![image(0, (100, 80)), message(1), message(2), message(3)];
    let app = open(&mut screen, entries);
    show(&mut screen, &[0], &png(100, 80));
    screen.draw(&app, None);
    assert_eq!(kept(previews_of(&mut screen)), items(&[0]));
    for y in [1, 2] {
        assert!(
            row_cells(&screen, y, 10)
                .iter()
                .all(|cell| cell.symbol() == " ")
        );
    }
    assert!(!any_sixel(&screen));
}

#[test]
fn a_moved_tmux_sixel_preview_is_drawn_again_on_a_cleared_screen() {
    let mut screen = screen(12, previews(4, tmux(), Limits::DEFAULT));
    // The image's six rows on pane rows 0 to 5, its preview on screen rows
    // 3 to 6.
    let entries = vec![message(0), message(1), image(2, (100, 80)), message(3)];
    let mut app = open(&mut screen, entries);
    show(&mut screen, &[2], &png(100, 80));
    // A frame that only adds a placement draws it at once.
    screen.draw(&app, None);
    assert!(sixel(screen.cell(2, 3)));
    assert!(!screen.again);

    // A message moves the preview up two rows: the first draw leaves it
    // out and asks for another on a cleared screen, which draws it.
    app.update(Msg::Timeline {
        timeline: TimelineId::main(RoomId::new(ROOM)),
        diffs: vec![TimelineDiff::PushBack(message(4))],
    });
    screen.draw(&app, None);
    assert!(screen.again);
    assert!(!any_sixel(&screen));
    screen.repaint();
    screen.draw(&app, None);
    assert!(!screen.again);
    assert!(sixel(screen.cell(2, 1)));
    assert!(!sixel(screen.cell(2, 3)));
    // Nothing moves: drawn where it is, without a cleared screen.
    screen.draw(&app, None);
    assert!(!screen.again);
}

#[test]
fn a_preview_for_another_cell_size_is_encoded_again_and_not_drawn_meanwhile() {
    let mut screen = screen(12, previews(4, tmux(), Limits::DEFAULT));
    let entries = vec![message(0), message(1), image(2, (100, 80)), message(3)];
    let app = open(&mut screen, entries);
    show(&mut screen, &[2], &png(100, 80));
    screen.draw(&app, None);
    assert!(sixel(screen.cell(2, 3)));

    // tmux counts cells of 10 by 40 pixels now: the image's 80 pixels take
    // 2 rows. The preview made for 4 rows of 20 pixels is left out, which
    // takes the frame to a cleared screen, and is encoded again.
    let taller = PreviewEncoder::TmuxSixel(FontSize::new(10, 40));
    previews_of(&mut screen).set_encoder(taller);
    screen.draw(&app, None);
    assert!(!any_sixel(&screen));
    assert!(screen.again);
    assert!(previews_of(&mut screen).is_queued(&item(2)));
    screen.repaint();
    screen.draw(&app, None);
    assert!(!any_sixel(&screen));
    assert!(!screen.again);

    // Drawn once encoded: the item's four rows above the last message, its
    // preview on screen rows 5 and 6.
    settle(previews_of(&mut screen));
    screen.draw(&app, None);
    assert!(sixel(screen.cell(2, 5)));
    assert!(!screen.again);
    let preview = previews_of(&mut screen).preview_of(&item(2)).unwrap();
    assert_eq!(preview.encoded.size(), Size::new(10, 2));
}

#[test]
fn a_preview_under_the_mention_menu_is_left_out() {
    let mut screen = screen(12, previews(2, halfblocks(), Limits::DEFAULT));
    // 77 columns of preview on screen rows 7 and 8, the menu over its
    // first 49 columns.
    let entries = vec![message(0), message(1), image(2, (770, 40))];
    let mut app = open(&mut screen, entries);
    show(&mut screen, &[2], &png(770, 40));
    screen.draw(&app, None);
    assert!(half_blocks(&row_cells(&screen, 7, 77)));

    press(&mut app, "i");
    let members = ["@bob:example.org", "@carol:example.org"]
        .into_iter()
        .map(|user| Member {
            user: UserId::new(user),
            display_name: None,
        })
        .collect();
    app.update(Msg::MembersLoaded {
        room: RoomId::new(ROOM),
        members,
    });
    app.update(Msg::MentionQuery {
        timeline: TimelineId::main(RoomId::new(ROOM)),
        query: Some(String::new()),
    });
    assert!(app.mention_menu().is_some());
    screen.draw(&app, None);
    // Beyond the menu, the preview's cells are blank: it was not drawn.
    for y in [7, 8] {
        assert_eq!(screen.cell(60, y).symbol(), " ", "row {y}");
    }
}

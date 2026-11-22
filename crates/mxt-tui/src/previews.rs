//! Image previews in the timeline (design, "Images in the timeline"): the
//! rows a preview reserves from the event's dimensions, one worker thread
//! that decodes, scales and encodes the fetched images, a bounded cache,
//! drawing with ratatui-image's `SlicedImage` (tmux's sixel drawn only
//! whole, and only on a screen that holds no image it has moved or left),
//! and the list of previews to fetch.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::io;
use std::mem;
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use image::{DynamicImage, RgbaImage, imageops};
use mxt_core::app::App;
use mxt_core::ids::TimelineId;
use mxt_core::msg::Msg;
use mxt_core::timeline::{ItemRef, MediaBytes};
use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui::widgets::Widget as _;
use ratatui_image::protocol::Protocol;
use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};
use ratatui_image::{FilterType, FontSize, Image, Resize};

use crate::image_view::{PreviewEncoder, decode, tmux_sixel};

/// What a not yet kept preview is counted as per pixel of its reserved
/// size, for the scaled image and again for its encoding.
const BYTES_PER_PIXEL: usize = 4;

/// A checked `ui.timeline.image_rows`: the most rows a preview in the
/// timeline takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageRows(u16);

impl ImageRows {
    /// The rows `ui.timeline.image_rows` accepts.
    pub const RANGE: RangeInclusive<u16> = 1..=50;

    /// `None` outside `RANGE`, so the configuration loader reports the
    /// value before the UI starts.
    pub fn new(rows: i64) -> Option<ImageRows> {
        u16::try_from(rows)
            .ok()
            .filter(|rows| ImageRows::RANGE.contains(rows))
            .map(ImageRows)
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

/// 8 rows.
impl Default for ImageRows {
    fn default() -> ImageRows {
        ImageRows(8)
    }
}

/// The cache's limits (design, "Memory"): how many previews it keeps and
/// their bytes, each one's scaled image and its encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) count: usize,
    pub(crate) bytes: usize,
}

impl Limits {
    pub(crate) const DEFAULT: Limits = Limits {
        count: 64,
        bytes: 64 * 1024 * 1024,
    };
}

/// What a preview is made for: its reserved size in cells, and whether the
/// event gave the image's dimensions, so that a loaded image is scaled up as
/// well as down to fill the size (a thumbnail is often smaller than the
/// image), and without them only down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) size: Size,
    pub(crate) scale: bool,
}

/// The rows previews take, for laying out the timeline's entries.
#[derive(Clone, Copy)]
pub(crate) struct PreviewLayout<'p> {
    rows: ImageRows,
    cell: FontSize,
    failed: &'p HashSet<ItemRef>,
}

impl PreviewLayout<'_> {
    /// The target of the preview of `item`, an image of `dimensions` under
    /// a line `width` columns wide; `None` once its preview failed.
    pub(crate) fn target(
        &self,
        item: &ItemRef,
        dimensions: Option<(u32, u32)>,
        width: u16,
    ) -> Option<Target> {
        if self.failed.contains(item) {
            return None;
        }
        let dimensions = dimensions.filter(|&(width, height)| width > 0 && height > 0);
        Some(Target {
            size: reserve(dimensions, width, self.rows, self.cell),
            scale: dimensions.is_some(),
        })
    }
}

/// Where the timeline places a preview this frame.
pub(crate) struct Placed<'a> {
    pub(crate) item: &'a ItemRef,
    /// The pane row of the preview's first row, negative above the pane.
    pub(crate) top: isize,
    pub(crate) target: Target,
}

/// A tmux sixel preview on the screen: the item and its area.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Placement {
    item: ItemRef,
    area: Rect,
}

/// The previews of the shown timeline.
pub(crate) struct Previews {
    rows: ImageRows,
    encoder: Arc<PreviewEncoder>,
    limits: Limits,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    /// The previews kept, the least recently drawn or finished first.
    kept: Vec<(ItemRef, Preview)>,
    /// The items whose job is with the worker.
    queued: HashSet<ItemRef>,
    /// The items whose preview could not be fetched, decoded or encoded,
    /// until the shown timeline changes.
    failed: HashSet<ItemRef>,
    /// The targets of the last frame that laid them out, in order (see
    /// [`Previews::choose_targets`]).
    targets: Vec<(ItemRef, Target)>,
    /// The last wanted list reported.
    reported: Option<(TimelineId, Vec<ItemRef>)>,
    /// The wanted list to report after this frame.
    report: Option<Msg>,
    /// The tmux sixel previews on the screen.
    on_screen: Vec<Placement>,
    /// The tmux sixel previews this frame places.
    placed: Vec<Placement>,
}

impl Previews {
    /// Starts the worker thread, which calls `wake` after each preview it
    /// finishes, so that the next frame takes it. Fails when the thread
    /// cannot start.
    pub(crate) fn new(
        rows: ImageRows,
        encoder: PreviewEncoder,
        limits: Limits,
        wake: Box<dyn Fn() + Send>,
    ) -> io::Result<Previews> {
        let (jobs, job_receiver) = mpsc::channel::<Job>();
        let (done_sender, done) = mpsc::channel();
        // Ends once `Previews`, and with it the job sender, is dropped.
        thread::Builder::new()
            .name("mxt-previews".to_owned())
            .spawn(move || {
                for job in job_receiver {
                    if done_sender.send(prepare(job)).is_err() {
                        break;
                    }
                    wake();
                }
            })?;
        Ok(Previews {
            rows,
            encoder: Arc::new(encoder),
            limits,
            jobs,
            done,
            kept: Vec::new(),
            queued: HashSet::new(),
            failed: HashSet::new(),
            targets: Vec::new(),
            reported: None,
            report: None,
            on_screen: Vec::new(),
            placed: Vec::new(),
        })
    }

    /// The encoder after the terminal resized: tmux may count cells in
    /// another size now. Kept previews made in another cell size are encoded
    /// again once they are targets.
    pub(crate) fn set_encoder(&mut self, encoder: PreviewEncoder) {
        self.encoder = Arc::new(encoder);
    }

    pub(crate) fn layout(&self) -> PreviewLayout<'_> {
        PreviewLayout {
            rows: self.rows,
            cell: self.encoder.cell(),
            failed: &self.failed,
        }
    }

    /// Another timeline is shown: its items may be wanted again.
    pub(crate) fn forget_failures(&mut self) {
        self.failed.clear();
    }

    /// `UiEffect::ShowPreview`: the bytes go to the worker for a target of
    /// the last frame; those of another item are dropped.
    pub(crate) fn show(&mut self, item: ItemRef, bytes: MediaBytes) {
        let Some(&(_, target)) = self.targets.iter().find(|(target, _)| *target == item) else {
            return;
        };
        if self.queued.contains(&item) || self.failed.contains(&item) || self.is_kept(&item) {
            return;
        }
        self.queue(item, Source::Bytes(bytes), target);
    }

    /// `UiEffect::PreviewFailed`.
    pub(crate) fn fail(&mut self, item: ItemRef) {
        self.failed.insert(item);
    }

    /// Takes the worker's finished previews into the cache and its failures.
    pub(crate) fn take_done(&mut self) {
        let mut changed = false;
        loop {
            match self.done.try_recv() {
                Ok(done) => {
                    self.accept(done);
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.failed.extend(self.queued.drain());
                    break;
                }
            }
        }
        if changed {
            self.settle_cache();
        }
    }

    /// Before each draw: forgets what the last draw placed.
    pub(crate) fn begin_frame(&mut self) {
        self.placed.clear();
    }

    /// The targets (design, "Fetching"): `on_pane`, the remote image items
    /// whose preview has not failed among the entries with a row on the
    /// pane, all of them, then those of `margin`, the entries around them by
    /// distance from the pane, until the next one would take the targets
    /// past the cache's limits, counting a kept preview's bytes and the
    /// estimate of one not kept yet.
    pub(crate) fn choose_targets<'a>(
        &self,
        on_pane: impl IntoIterator<Item = (&'a ItemRef, Target)>,
        margin: impl IntoIterator<Item = (&'a ItemRef, Target)>,
    ) -> Vec<(ItemRef, Target)> {
        let cell = self.encoder.cell();
        let cost = |item: &ItemRef, target: Target| match self.preview_of(item) {
            Some(preview) => preview.bytes(),
            None => estimate(target.size, cell),
        };
        let mut targets = Vec::new();
        let mut bytes = 0;
        for (item, target) in on_pane {
            bytes += cost(item, target);
            targets.push((item.clone(), target));
        }
        for (item, target) in margin {
            let more = bytes + cost(item, target);
            if targets.len() >= self.limits.count || more > self.limits.bytes {
                break;
            }
            bytes = more;
            targets.push((item.clone(), target));
        }
        targets
    }

    /// Draws the kept previews of `placed` into `area`, the rows of the
    /// timeline's entries from the column of their text, leaving out those
    /// with no row on it, those that meet a rectangle of `covered` (the
    /// overlays) and those made for another size, which are encoded again.
    /// tmux's sixel is drawn only while all its rows are on the pane, and in
    /// a frame that leaves no tmux sixel preview of the screen out or moves
    /// it (see [`Previews::finish_frame`]).
    pub(crate) fn draw<'a>(
        &mut self,
        buffer: &mut Buffer,
        area: Rect,
        placed: impl IntoIterator<Item = Placed<'a>>,
        covered: &[Rect],
    ) {
        let cell = self.encoder.cell();
        let height = signed(area.height);
        for slot in placed {
            let Some(index) = self.kept.iter().position(|(item, _)| item == slot.item) else {
                continue;
            };
            let preview = &self.kept[index].1;
            if !preview.made_for(slot.target, cell) {
                continue;
            }
            let size = preview.encoded.size();
            let (top, bottom) = (slot.top, slot.top + signed(size.height));
            let (Ok(first), Ok(end)) =
                (u16::try_from(top.max(0)), u16::try_from(bottom.min(height)))
            else {
                continue;
            };
            if first >= end {
                continue;
            }
            let visible = Rect {
                x: area.x,
                y: area.y + first,
                width: size.width.min(area.width),
                height: end - first,
            };
            if covered.iter().any(|covered| covered.intersects(visible)) {
                continue;
            }
            match &preview.encoded {
                Encoded::Sliced(sliced) => {
                    let Ok(y) = i16::try_from(top) else {
                        continue;
                    };
                    SlicedImage::new(sliced, SignedPosition { x: 0, y }).render(area, buffer);
                }
                // The sixel is the whole image: past the pane's bottom it
                // would cover the composer, and it cannot start above the
                // pane.
                Encoded::TmuxSixel(_) if top < 0 || bottom > height => continue,
                Encoded::TmuxSixel(_) => self.placed.push(Placement {
                    item: slot.item.clone(),
                    area: visible,
                }),
            }
            let drawn = self.kept.remove(index);
            self.kept.push(drawn);
        }
        if self.placement_gone() {
            return;
        }
        for placement in &self.placed {
            if let Some(Preview {
                encoded: Encoded::TmuxSixel(sixel),
                ..
            }) = self.preview_of(&placement.item)
            {
                Image::new(sixel).render(placement.area, buffer);
            }
        }
    }

    /// After the frame's layout: `targets` from [`Previews::choose_targets`]
    /// for the timeline `app` shows. The cache drops what is past its limits
    /// and no target, kept targets made for another size or cell size are
    /// encoded again, and the wanted list, the targets the cache neither
    /// keeps nor is decoding (the failed ones are no targets), is reported
    /// when it differs from the last report.
    pub(crate) fn set_targets(&mut self, app: &App, targets: Vec<(ItemRef, Target)>) {
        self.targets = targets;
        self.settle_cache();
        let wanted =
            |(item, _): &&(ItemRef, Target)| !self.queued.contains(item) && !self.is_kept(item);
        let same = self.reported.as_ref().is_some_and(|(timeline, items)| {
            app.shows(timeline)
                && items
                    .iter()
                    .eq(self.targets.iter().filter(wanted).map(|(item, _)| item))
        });
        if same {
            return;
        }
        let Some(timeline) = app.shown_timeline() else {
            return;
        };
        let items: Vec<ItemRef> = self
            .targets
            .iter()
            .filter(wanted)
            .map(|(item, _)| item.clone())
            .collect();
        self.reported = Some((timeline.clone(), items.clone()));
        self.report = Some(Msg::PreviewsWanted { timeline, items });
    }

    /// After each draw: whether a tmux sixel preview on the screen is left
    /// out or moved, so that the frame, drawn without tmux's sixel, must be
    /// drawn again on a cleared screen (tmux keeps an image under text that
    /// is not ASCII). Otherwise the frame's placements are on the screen.
    pub(crate) fn finish_frame(&mut self) -> bool {
        if self.placement_gone() {
            return true;
        }
        mem::swap(&mut self.on_screen, &mut self.placed);
        false
    }

    /// The screen was cleared, which drops every image.
    pub(crate) fn screen_cleared(&mut self) {
        self.on_screen.clear();
    }

    /// `Msg::PreviewsWanted` once after a frame whose wanted list changed.
    pub(crate) fn take_report(&mut self) -> Option<Msg> {
        self.report.take()
    }

    fn placement_gone(&self) -> bool {
        self.on_screen
            .iter()
            .any(|placement| !self.placed.contains(placement))
    }

    fn preview_of(&self, item: &ItemRef) -> Option<&Preview> {
        self.kept
            .iter()
            .find_map(|(kept, preview)| (kept == item).then_some(preview))
    }

    fn is_kept(&self, item: &ItemRef) -> bool {
        self.kept.iter().any(|(kept, _)| kept == item)
    }

    fn queue(&mut self, item: ItemRef, source: Source, target: Target) {
        let job = Job {
            item: item.clone(),
            source,
            target,
            encoder: Arc::clone(&self.encoder),
        };
        if self.jobs.send(job).is_ok() {
            self.queued.insert(item);
        } else {
            self.failed.insert(item);
        }
    }

    fn accept(&mut self, done: Done) {
        match done {
            Done::Ready(item, preview) => {
                self.queued.remove(&item);
                self.kept.retain(|(kept, _)| *kept != item);
                self.kept.push((item, preview));
            }
            Done::Failed(item) => {
                self.queued.remove(&item);
                self.failed.insert(item);
            }
        }
    }

    /// Drops the least recently drawn previews past the limits, never a
    /// target, so that a window holding more previews than the limits keeps
    /// them instead of fetching and dropping them in turn; and sends the
    /// targets made for another size or cell size to the worker, which
    /// scales and encodes them again from their scaled image.
    fn settle_cache(&mut self) {
        loop {
            let bytes: usize = self.kept.iter().map(|(_, preview)| preview.bytes()).sum();
            if self.kept.len() <= self.limits.count && bytes <= self.limits.bytes {
                break;
            }
            let targets = &self.targets;
            let Some(oldest) = self
                .kept
                .iter()
                .position(|(item, _)| !targets.iter().any(|(target, _)| target == item))
            else {
                break;
            };
            self.kept.remove(oldest);
        }
        let cell = self.encoder.cell();
        let mut stale: Vec<(usize, Target)> = self
            .targets
            .iter()
            .filter_map(|(item, target)| {
                let index = self.kept.iter().position(|(kept, _)| kept == item)?;
                let preview = &self.kept[index].1;
                (!preview.made_for(*target, cell)).then_some((index, *target))
            })
            .collect();
        // From the last index, so that the others stay where they are.
        stale.sort_unstable_by_key(|&(index, _)| Reverse(index));
        for (index, target) in stale {
            let (item, preview) = self.kept.remove(index);
            self.queue(item, Source::Scaled(preview.scaled), target);
        }
    }
}

/// The reserved size of a preview in cells (design, "Layout"): a box of
/// `rows` rows by `width` columns of `cell` pixels; with the image's
/// `dimensions`, ratatui-image's `Resize::Fit` of the image into the box,
/// in proportion, never larger than the image, rounded up to whole cells
/// (ratatui-image `src/lib.rs:440-447, 526-545`); without them the box.
pub(crate) fn reserve(
    dimensions: Option<(u32, u32)>,
    width: u16,
    rows: ImageRows,
    cell: FontSize,
) -> Size {
    let area = Size::new(width, rows.get());
    let Some((image_width, image_height)) =
        dimensions.filter(|&(width, height)| width > 0 && height > 0)
    else {
        return area;
    };
    let box_width = u32::from(width) * u32::from(cell.width);
    let box_height = u32::from(rows.get()) * u32::from(cell.height);
    let (fit_width, fit_height) = fit(
        image_width,
        image_height,
        box_width.min(image_width),
        box_height.min(image_height),
    );
    let size = cells(fit_width, fit_height, cell);
    Size::new(size.width.min(area.width), size.height.min(area.height))
}

/// `width` by `height` pixels scaled in proportion to fit `max_width` by
/// `max_height`, at least one pixel each way: ratatui-image's private
/// `fit_area_proportionally` (`src/lib.rs:556-574`), without its branch for
/// sides past `u16::MAX` pixels, which [`reserve`] cuts to the box anyway.
fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let ratio = f64::min(
        f64::from(max_width) / f64::from(width),
        f64::from(max_height) / f64::from(height),
    );
    // At most the side itself: `ratio` is at most 1 here.
    let scale = |side: u32| ((f64::from(side) * ratio).round() as u32).max(1);
    (scale(width), scale(height))
}

/// Pixels in whole cells, rounded up: ratatui-image's private
/// `round_pixel_size_to_cells` (`src/lib.rs:540-545`).
fn cells(width: u32, height: u32, cell: FontSize) -> Size {
    let count = |pixels: u32, side: u16| {
        u16::try_from(pixels.div_ceil(u32::from(side.max(1)))).unwrap_or(u16::MAX)
    };
    Size::new(count(width, cell.width), count(height, cell.height))
}

/// The bytes a preview not kept yet is counted as: its reserved size's
/// pixels at 4 bytes each for the scaled image and the same again for its
/// encoding.
pub(crate) fn estimate(size: Size, cell: FontSize) -> usize {
    pixels(size, cell) * 2 * BYTES_PER_PIXEL
}

fn pixels(size: Size, cell: FontSize) -> usize {
    usize::from(size.width)
        * usize::from(cell.width)
        * usize::from(size.height)
        * usize::from(cell.height)
}

fn signed(rows: u16) -> isize {
    isize::try_from(rows).unwrap_or(isize::MAX)
}

/// A preview for the worker to make.
struct Job {
    item: ItemRef,
    source: Source,
    target: Target,
    encoder: Arc<PreviewEncoder>,
}

enum Source {
    /// The fetched file.
    Bytes(MediaBytes),
    /// A kept preview's scaled image, for another size or cell size.
    Scaled(DynamicImage),
}

/// What the worker answers a job with.
enum Done {
    Ready(ItemRef, Preview),
    Failed(ItemRef),
}

/// A preview as the cache keeps it.
struct Preview {
    /// The image scaled to the target, unpadded, in RGBA: what it is
    /// encoded from again for another size.
    scaled: DynamicImage,
    encoded: Encoded,
    /// What it was made for.
    target: Target,
    cell: FontSize,
}

impl Preview {
    /// The scaled image's bytes and its encoding's, counted as 4 bytes per
    /// pixel of the encoded image, so that a kept preview never counts more
    /// than its estimate.
    fn bytes(&self) -> usize {
        self.scaled.as_bytes().len() + pixels(self.encoded.size(), self.cell) * BYTES_PER_PIXEL
    }

    /// Whether it was made for `target` in cells of `cell` pixels.
    fn made_for(&self, target: Target, cell: FontSize) -> bool {
        self.target == target && (self.cell.width, self.cell.height) == (cell.width, cell.height)
    }
}

enum Encoded {
    /// ratatui-image's sliced image, drawn with `SlicedImage`.
    Sliced(SlicedProtocol),
    /// tmux's sixel, drawn whole with `Image`.
    TmuxSixel(Protocol),
}

impl Encoded {
    fn size(&self) -> Size {
        match self {
            Encoded::Sliced(sliced) => sliced.size(),
            Encoded::TmuxSixel(sixel) => sixel.size(),
        }
    }
}

/// The worker's work on one job: the fetched file decoded with the message
/// view's `decode` within the target's pixels (the format sniffed, the
/// memory limit, the Exif orientation), or the kept scaled image, scaled to
/// fit the target with a triangle filter (`Resize::Scale` with the image's
/// dimensions, `Resize::Fit` without), padded to whole cells as
/// `Resize::resize` does (ratatui-image `src/lib.rs:413-437`) and encoded
/// for the protocol. A decoding or encoding error fails the preview.
fn prepare(job: Job) -> Done {
    let Job {
        item,
        source,
        target,
        encoder,
    } = job;
    match make(source, target, &encoder) {
        Some(preview) => Done::Ready(item, preview),
        None => Done::Failed(item),
    }
}

fn make(source: Source, target: Target, encoder: &PreviewEncoder) -> Option<Preview> {
    let cell = encoder.cell();
    let image = match source {
        Source::Bytes(bytes) => {
            let width = u32::from(target.size.width) * u32::from(cell.width);
            let height = u32::from(target.size.height) * u32::from(cell.height);
            decode(bytes.as_bytes(), width, height).ok()?
        }
        Source::Scaled(image) => image,
    };
    let resize = if target.scale {
        Resize::Scale(Some(FilterType::Triangle))
    } else {
        Resize::Fit(Some(FilterType::Triangle))
    };
    let size = resize.size_for(&image, cell, target.size);
    let width = u32::from(size.width) * u32::from(cell.width);
    let height = u32::from(size.height) * u32::from(cell.height);
    // `Resize::resize` before its padding, which the scaled image is kept
    // without, so that scaling it again scales the image alone.
    let scaled = image
        .resize(width, height, FilterType::Triangle)
        .into_rgba8();
    let mut padded = RgbaImage::new(width, height);
    imageops::replace(&mut padded, &scaled, 0, 0);
    let padded = DynamicImage::ImageRgba8(padded);
    let encoded = match encoder {
        PreviewEncoder::Picker(picker) => Encoded::Sliced(
            SlicedProtocol::new_with_resize(picker, padded, size, Resize::Fit(None)).ok()?,
        ),
        PreviewEncoder::TmuxSixel(cell) => {
            Encoded::TmuxSixel(tmux_sixel(&padded, *cell, &Resize::Fit(None), size).ok()?)
        }
    };
    Some(Preview {
        scaled: DynamicImage::ImageRgba8(scaled),
        encoded,
        target,
        cell,
    })
}

#[cfg(test)]
mod tests;

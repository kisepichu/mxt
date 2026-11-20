//! Images in the message view (design, "Files and images"): decoded with
//! `image`, scaled down to the terminal, kept for the last few items, and
//! drawn with ratatui-image in the graphics protocol chosen for the terminal
//! (see [`crate::image_protocol`]): sixel, kitty, iTerm2, else half blocks,
//! and inside tmux sixel that tmux stores in the pane.

use std::io::{self, Cursor, Write as _};
use std::thread;
use std::time::{Duration, Instant};

use image::metadata::Orientation;
use image::{
    DynamicImage, ImageDecoder as _, ImageFormat, ImageReader, ImageResult, Limits, RgbaImage,
};
use mxt_core::settings::ImageProtocol;
use mxt_core::timeline::ItemRef;
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui_image::errors::Errors;
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::{Capability, Picker};
use ratatui_image::protocol::sixel::Sixel;
use ratatui_image::protocol::{Protocol, StatefulProtocol};
use ratatui_image::{FilterType, FontSize, Image, Resize, StatefulImage};
use rustix::termios;

use crate::image_protocol::{self, Choice, Env};

/// How many decoded images are kept.
const KEPT: usize = 4;
/// The most a decoder may allocate, so an image that is small as a file but
/// huge in pixels fails at once instead of freezing the UI. A 24 megapixel
/// image in RGBA fits.
const MAX_DECODE_ALLOC: u64 = 128 * 1024 * 1024;
/// How long [`release_query_thread`] waits for tmux's answer.
const RELEASE_WAIT: Duration = Duration::from_millis(100);
/// The rows of pixels in a sixel band.
const SIXEL_BAND: u32 = 6;

pub(crate) struct Images {
    encoder: Encoder,
    kept: Recent<ItemRef, Kept>,
    /// An encoding failure found by [`Images::draw`], until
    /// [`Images::take_failure`].
    failure: Option<String>,
}

/// How [`Images`] prepares a decoded image for the terminal.
enum Encoder {
    /// For ratatui-image, in the protocol of its picker.
    Picker(Picker),
    /// As sixel for tmux to store, in tmux's cell size (see [`TmuxSixel`]),
    /// which [`Images::resized`] takes again.
    TmuxSixel(FontSize),
}

impl Encoder {
    /// The cell size in pixels that images are scaled for.
    fn font_size(&self) -> FontSize {
        match self {
            Encoder::Picker(picker) => picker.font_size(),
            Encoder::TmuxSixel(cell) => *cell,
        }
    }
}

/// A decoded image, scaled down, prepared for the terminal.
struct Kept {
    image: Prepared,
    /// Whether an encoding failure of the image was found already.
    failed: bool,
}

enum Prepared {
    /// ratatui-image's state, which scales and encodes the image while
    /// drawing.
    Picker(StatefulProtocol),
    TmuxSixel(TmuxSixel),
}

impl Images {
    /// Chooses the graphics protocol: `forced`, or for `None` (`auto`) the
    /// one for the terminal's name, and otherwise ratatui-image's; inside
    /// tmux, sixel for tmux to store where tmux can, else half blocks. Unless
    /// the choice is half blocks or sixel for tmux, ratatui-image then asks
    /// the terminal for its graphics protocols and cell size. The terminal
    /// must be in raw mode, and nothing else may read its input yet: the
    /// answers arrive there.
    pub(crate) fn query(forced: Option<ImageProtocol>) -> io::Result<Images> {
        let choice = image_protocol::choose(
            forced,
            &Env::read(),
            image_protocol::query_terminal,
            image_protocol::query_tmux,
        );
        let encoder = match choice {
            Choice::Halfblocks => Encoder::Picker(Picker::halfblocks()),
            Choice::Detected => Encoder::Picker(detect()?),
            Choice::ForTerminal(protocol) => {
                let mut picker = detect()?;
                if cell_size_measured(&picker) {
                    picker.set_protocol_type(protocol);
                }
                Encoder::Picker(picker)
            }
            Choice::Forced(protocol) => {
                let mut picker = detect()?;
                picker.set_protocol_type(protocol);
                Encoder::Picker(picker)
            }
            Choice::TmuxSixel { cell } => Encoder::TmuxSixel(cell.into()),
        };
        Ok(Images {
            encoder,
            kept: Recent::new(KEPT),
            failure: None,
        })
    }

    /// Whether images are sixel that tmux stores in the pane: tmux drops
    /// such an image when ASCII text is written over it, but not for other
    /// characters, while a cleared screen drops every image.
    pub(crate) fn stored_by_tmux(&self) -> bool {
        matches!(self.encoder, Encoder::TmuxSixel(_))
    }

    /// After the terminal resized, takes tmux's cell size again for sixel
    /// that tmux stores. tmux resizes the window when another client with
    /// another window size takes it over (with tmux's default `window-size
    /// latest`, the client that attached or typed last), and from then on
    /// counts an image's cells in that client's cell size. ratatui redraws
    /// every cell after a resize, so images are encoded again in the new
    /// cell size. When it cannot be read, tmux's cell size stays.
    pub(crate) fn resized(&mut self) {
        if let Encoder::TmuxSixel(cell) = &mut self.encoder
            && let Some(read) = image_protocol::tmux_cell()
        {
            *cell = read.into();
        }
    }

    /// Decodes `bytes`, the image of `item`, unless it is kept already, and
    /// keeps it scaled down to fit `screen` (the terminal's size in cells).
    pub(crate) fn insert(&mut self, item: ItemRef, bytes: &[u8], screen: Size) -> ImageResult<()> {
        if self.kept.get_mut(&item).is_some() {
            return Ok(());
        }
        let font = self.encoder.font_size();
        let width = u32::from(screen.width) * u32::from(font.width);
        let height = u32::from(screen.height) * u32::from(font.height);
        let image = decode(bytes, width, height)?;
        let image = match &self.encoder {
            Encoder::Picker(picker) => Prepared::Picker(picker.new_resize_protocol(image)),
            Encoder::TmuxSixel(_) => Prepared::TmuxSixel(TmuxSixel {
                image,
                encoded: None,
            }),
        };
        self.kept.insert(
            item,
            Kept {
                image,
                failed: false,
            },
        );
        Ok(())
    }

    /// Draws the image of `item` in the top left of `area`, scaled down to
    /// fit, once it is kept. The image is encoded while drawing; the first
    /// failure of each image is kept for [`Images::take_failure`].
    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, item: &ItemRef) {
        // For sixel that tmux stores, tmux's cell size.
        let font = self.encoder.font_size();
        let Some(kept) = self.kept.get_mut(item) else {
            return;
        };
        let resize = Resize::Fit(Some(FilterType::Triangle));
        let failure = match &mut kept.image {
            Prepared::Picker(image) => {
                frame.render_stateful_widget(StatefulImage::new().resize(resize), area, image);
                image
                    .last_encoding_result()
                    .and_then(Result::err)
                    .map(|error| error.to_string())
            }
            Prepared::TmuxSixel(image) => match image.encoded(&resize, area.as_size(), font) {
                Ok(sixel) => {
                    frame.render_widget(Image::new(sixel), area);
                    None
                }
                Err(error) => (!kept.failed).then(|| error.to_string()),
            },
        };
        if let Some(error) = failure
            && !kept.failed
        {
            kept.failed = true;
            self.failure = Some(error);
        }
    }

    /// The encoding failure found by the draws since the last call.
    pub(crate) fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }
}

/// An image for tmux to store in the pane as sixel. tmux places it at the
/// pane's cursor, keeps it within the pane, draws it for each client whose
/// terminal draws sixel (scaled to that terminal's cell size), and drops
/// it once ASCII text, or an erase, is written over it (tmux's image.c and
/// screen-write.c).
struct TmuxSixel {
    image: DynamicImage,
    /// What the image was last encoded for, and its sixel.
    encoded: Option<(Fit, Result<Protocol, Errors>)>,
}

/// The size in cells of an image, and tmux's cell size (width, height) in
/// pixels.
type Fit = (Size, (u16, u16));

impl TmuxSixel {
    /// The sixel of the image scaled down by `resize` to fit `area` in
    /// cells of `cell` pixels, tmux's cell size, encoded again only when its
    /// size in cells or tmux's cell size changes: in another cell size, tmux
    /// would count the same pixels as another number of cells.
    fn encoded(
        &mut self,
        resize: &Resize,
        area: Size,
        cell: FontSize,
    ) -> &Result<Protocol, Errors> {
        let key: Fit = (
            resize.size_for(&self.image, cell, area),
            (cell.width, cell.height),
        );
        let encoded = match self.encoded.take() {
            Some((encoded, sixel)) if encoded == key => (encoded, sixel),
            _ => (key, tmux_sixel(&self.image, cell, resize, key.0)),
        };
        &self.encoded.insert(encoded).1
    }
}

/// `image` scaled by `resize` to `size` in cells of `cell` pixels, as
/// ratatui-image scales it, in sixel without the passthrough wrapper that
/// ratatui-image puts around it in tmux (its picker sees tmux in `TERM`
/// and has no setter), so that tmux reads the image itself. Only whole
/// sixel bands of 6 rows of pixels are kept: tmux counts an image's height
/// in whole bands, so an image whose height is not a multiple of 6 would
/// take a row more than `size`, outside the cells kept for it. Text written
/// there, such as the blanks of the view's first frame, makes tmux drop the
/// image, and on the pane's last row tmux scrolls the pane instead. This
/// takes at most 5 rows of pixels off the bottom.
fn tmux_sixel(
    image: &DynamicImage,
    cell: FontSize,
    resize: &Resize,
    size: Size,
) -> Result<Protocol, Errors> {
    let fitted = resize.resize(image, cell, size, None).into_rgba8();
    let (width, height) = fitted.dimensions();
    let height = height - height % SIXEL_BAND;
    let mut pixels = fitted.into_raw();
    pixels.truncate(width as usize * height as usize * 4);
    let fitted = RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| Errors::Sixel("the image lost pixels".to_owned()))?;
    Sixel::new(fitted.into(), size, false).map(Protocol::Sixel)
}

/// `bytes` in the format they start with, decoded within
/// [`MAX_DECODE_ALLOC`], scaled down to fit `width` x `height` pixels as
/// shown, and then turned by their Exif orientation (phone cameras store
/// portrait photos as landscape ones with orientation 6 or 8), so that a
/// turn by 90 degrees copies the small image. Only JPEG and PNG are turned;
/// other formats, and an orientation that cannot be read, stay as stored.
fn decode(bytes: &[u8], width: u32, height: u32) -> ImageResult<DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let format = reader.format();
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder()?;
    // What `ImageReader::decode` does, which gives no access to the
    // decoder: the image's own buffer counts against the limit too.
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    // Only where reading the Exif data allocates no more than the file
    // holds: a JPEG's APP1 segment is at most 64 KiB, and png read `eXIf`
    // within its limits already. image-webp allocates the size an `EXIF`
    // chunk claims, outside the limit, so a 66-byte WebP could make mxt
    // abort wherever a 4 GiB allocation is refused.
    let orientation = if matches!(format, Some(ImageFormat::Jpeg | ImageFormat::Png)) {
        decoder.orientation().unwrap_or(Orientation::NoTransforms)
    } else {
        Orientation::NoTransforms
    };
    let image = DynamicImage::from_decoder(decoder)?;
    let sideways = matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    // The bounds for the image as stored.
    let (width, height) = if sideways {
        (height, width)
    } else {
        (width, height)
    };
    let mut image = if image.width() > width || image.height() > height {
        image.thumbnail(width, height)
    } else {
        image
    };
    image.apply_orientation(orientation);
    Ok(image)
}

/// ratatui-image's query for the terminal's graphics protocols and cell
/// size. When the answers do not come in time, ratatui-image guesses from
/// the environment (iTerm2 hints, the cell size from the window's size in
/// pixels) and else uses half blocks. A query that fails uses half blocks.
fn detect() -> io::Result<Picker> {
    let options = QueryStdioOptions::default();
    let timeout = options.timeout;
    let started = Instant::now();
    let picker =
        Picker::from_query_stdio_with_options(options).unwrap_or_else(|_| Picker::halfblocks());
    if picker.tmux_detected() && started.elapsed() >= timeout {
        release_query_thread()?;
    }
    Ok(picker)
}

/// Whether `picker`'s cell size was measured: answered by the terminal, or
/// taken from the window's size in pixels, as ratatui-image does when the
/// terminal does not answer. Otherwise it is a guess, ratatui-image's or
/// that of the half blocks [`detect`] falls back to, and ratatui-image
/// itself then uses only half blocks: the other protocols draw the image in
/// pixels, which must fill the cells it is laid out in.
fn cell_size_measured(picker: &Picker) -> bool {
    let window = termios::tcgetwinsize(io::stdout())
        .ok()
        .and_then(|size| image_protocol::window_cell(&size));
    let font = picker.font_size();
    measured(picker.capabilities(), (font.width, font.height), window)
}

/// Whether the cell size `font` (width, height) was measured, given the
/// `capabilities` ratatui-image's query found and `window`, the cell size
/// from the window's size in pixels, computed as ratatui-image does.
fn measured(capabilities: &[Capability], font: (u16, u16), window: Option<(u16, u16)>) -> bool {
    capabilities
        .iter()
        .any(|capability| matches!(capability, Capability::CellSize(Some(_))))
        || window == Some(font)
}

/// A workaround for ratatui-image's query thread, which reads stdin without
/// a timeout. After a query that timed out, it still takes the next input,
/// usually the first key press, and then restores the terminal mode it found,
/// so it must end while mxt is in raw mode. In tmux the whole query goes
/// through passthrough, so it times out whenever no terminal answers it: in
/// a pane no client shows, or over SSH from a local tmux without
/// `allow-passthrough`. tmux answers a plain DSR itself, so the thread takes
/// that answer and ends before the event stream exists. An answer that takes
/// longer than the wait (a slow link) still reaches the thread instead of a
/// key, unless a key comes first. After a query that was slow but answered,
/// no thread is left, and crossterm drops the answer.
fn release_query_thread() -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(b"\x1b[5n")?;
    stdout.flush()?;
    thread::sleep(RELEASE_WAIT);
    Ok(())
}

/// The values of the last `capacity` keys inserted or looked up.
struct Recent<K, V> {
    capacity: usize,
    /// The least recently used first.
    entries: Vec<(K, V)>,
}

impl<K: PartialEq, V> Recent<K, V> {
    fn new(capacity: usize) -> Recent<K, V> {
        Recent {
            capacity,
            entries: Vec::with_capacity(capacity),
        }
    }

    /// The value of `key`, which becomes the most recently used.
    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(index);
        self.entries.push(entry);
        self.entries.last_mut().map(|(_, value)| value)
    }

    /// Keeps `value` as the most recently used, in place of the value of the
    /// same key or else of the least recently used key when full.
    fn insert(&mut self, key: K, value: V) {
        if let Some(index) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(index);
        } else if self.entries.len() == self.capacity {
            self.entries.remove(0);
        }
        self.entries.push((key, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::webp::WebPEncoder;
    use image::{ImageEncoder as _, ImageError, ImageFormat};

    fn keys(recent: &Recent<u32, &str>) -> Vec<u32> {
        recent.entries.iter().map(|(key, _)| *key).collect()
    }

    #[test]
    fn the_least_recently_used_image_is_dropped_when_full() {
        let mut recent = Recent::new(2);
        recent.insert(1, "a");
        recent.insert(2, "b");
        // Looking 1 up makes 2 the least recently used.
        assert_eq!(recent.get_mut(&1), Some(&mut "a"));
        recent.insert(3, "c");
        assert_eq!(keys(&recent), [1, 3]);
        assert_eq!(recent.get_mut(&2), None);
    }

    #[test]
    fn inserting_a_kept_key_replaces_its_value_without_dropping_another() {
        let mut recent = Recent::new(2);
        recent.insert(1, "a");
        recent.insert(2, "b");
        recent.insert(1, "A");
        assert_eq!(keys(&recent), [2, 1]);
        assert_eq!(recent.get_mut(&1), Some(&mut "A"));
    }

    #[test]
    fn only_a_measured_cell_size_counts() {
        let window = Some((8, 16));
        // The half blocks `detect` falls back to after a failed query guess
        // 10 by 20, whatever the window says.
        assert!(!measured(&[], (10, 20), window));
        // ratatui-image's own fallback takes the window's cell size.
        assert!(measured(&[], (8, 16), window));
        assert!(!measured(&[], (10, 20), None));
        let answered = [Capability::CellSize(Some((9, 18)))];
        assert!(measured(&answered, (9, 18), None));
        assert!(!measured(&[Capability::CellSize(None)], (10, 20), None));
    }

    /// The width and height in pixels in the raster attributes
    /// (`"Pan;Pad;Ph;Pv`) of the sixel image in `data`.
    fn raster(data: &str) -> (u32, u32) {
        let (_, attributes) = data.split_once("q\"").unwrap();
        let attributes = &attributes[..attributes.find('#').unwrap()];
        let values: Vec<u32> = attributes.split(';').map(|v| v.parse().unwrap()).collect();
        (values[2], values[3])
    }

    #[test]
    fn sixel_for_tmux_is_not_wrapped_and_takes_exactly_its_cells() {
        // A portrait photo, which fills the height of its area: 7 rows of
        // 20 pixels, 140 pixels, which are 23 sixel bands and 2 rows.
        let mut image = TmuxSixel {
            image: DynamicImage::new_rgb8(300, 900),
            encoded: None,
        };
        let resize = Resize::Fit(Some(FilterType::Triangle));
        let cell = FontSize::new(10, 20);
        let Ok(Protocol::Sixel(sixel)) = image.encoded(&resize, Size::new(40, 7), cell) else {
            panic!("not encoded as sixel");
        };
        assert_eq!(sixel.size, Size::new(5, 7));
        // No passthrough (`ESC P tmux;` with every ESC doubled), so that tmux
        // reads the image itself.
        assert!(!sixel.is_tmux);
        assert!(!sixel.data.contains("tmux;") && !sixel.data.contains("\x1b\x1b"));
        // tmux counts the image's height in whole bands, and its cells
        // rounded up (`sixel_size_in_cells` in tmux's image-sixel.c): 140
        // pixels would count as 144, a row more.
        let (width, height) = raster(&sixel.data);
        assert_eq!((width, height), (50, 138));
        assert_eq!(height.div_ceil(20), 7);
    }

    #[test]
    fn sixel_for_tmux_is_encoded_again_in_another_cell_size() {
        // A portrait photo that takes 5x7 cells both at 10x20 and at 16x32
        // pixels. A sixel kept for 5x7 cells alone would stay 138 pixels
        // high, which tmux at 16x32 counts as 5 rows.
        let mut image = TmuxSixel {
            image: DynamicImage::new_rgb8(300, 900),
            encoded: None,
        };
        let resize = Resize::Fit(Some(FilterType::Triangle));
        for (cell, pixels) in [
            ((10, 20), (50, 138)),
            ((16, 32), (80, 222)),
            ((10, 20), (50, 138)),
        ] {
            let Ok(Protocol::Sixel(sixel)) = image.encoded(&resize, Size::new(40, 7), cell.into())
            else {
                panic!("not encoded as sixel");
            };
            assert_eq!(sixel.size, Size::new(5, 7), "{cell:?}");
            assert_eq!(raster(&sixel.data), pixels, "{cell:?}");
            assert_eq!(pixels.1.div_ceil(u32::from(cell.1)), 7);
        }
    }

    /// Appends a PNG chunk: its length, type, data and CRC-32.
    fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        let mut crc = !0u32;
        for &byte in kind.iter().chain(data) {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        png.extend(u32::try_from(data.len()).unwrap().to_be_bytes());
        png.extend(kind);
        png.extend(data);
        png.extend((!crc).to_be_bytes());
    }

    /// `bytes` decoded for a terminal larger than any image.
    fn decode_unscaled(bytes: &[u8]) -> ImageResult<DynamicImage> {
        decode(bytes, u32::MAX, u32::MAX)
    }

    #[test]
    fn decoding_is_bounded_by_the_pixels_not_the_file_size() {
        let mut small = Vec::new();
        DynamicImage::new_rgb8(3, 2)
            .write_to(&mut Cursor::new(&mut small), ImageFormat::Png)
            .unwrap();
        let image = decode_unscaled(&small).unwrap();
        assert_eq!((image.width(), image.height()), (3, 2));

        // 57 bytes for 8192x8192 in RGBA: 256 MiB decoded, under `image`'s
        // default limit but over ours.
        let mut huge = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::new();
        header.extend(8192u32.to_be_bytes());
        header.extend(8192u32.to_be_bytes());
        // 8 bits per channel, RGBA, the only compression and filter
        // methods, no interlacing.
        header.extend([8, 6, 0, 0, 0]);
        chunk(&mut huge, b"IHDR", &header);
        chunk(&mut huge, b"IDAT", &[]);
        chunk(&mut huge, b"IEND", &[]);
        let error = decode_unscaled(&huge).unwrap_err();
        assert!(matches!(error, ImageError::Limits(_)), "{error:?}");
    }

    /// An Exif TIFF block with orientation 6, which turns the image by 90
    /// degrees: as phone cameras store a portrait photo, landscape.
    const TURNED_BY_90: &[u8] = &[
        b'M', b'M', 0, 42, 0, 0, 0, 8, // big-endian TIFF header, IFD at 8
        0, 1, // one entry
        0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, // orientation, one short: 6
        0, 0, 0, 0, // no next IFD
    ];

    #[test]
    fn a_photo_is_turned_by_its_exif_orientation_after_scaling() {
        let mut stored = Vec::new();
        DynamicImage::new_rgb8(30, 20)
            .write_to(&mut Cursor::new(&mut stored), ImageFormat::Jpeg)
            .unwrap();
        let image = decode_unscaled(&stored).unwrap();
        assert_eq!((image.width(), image.height()), (30, 20));

        let app1 = [&[0xFF, 0xE1, 0, 34][..], b"Exif\0\0", TURNED_BY_90].concat();
        let turned = [&stored[..2], &app1, &stored[2..]].concat();
        let image = decode_unscaled(&turned).unwrap();
        assert_eq!((image.width(), image.height()), (20, 30));
        // Fitted as shown: 20x30 into 10 pixels wide.
        let image = decode(&turned, 10, 100).unwrap();
        assert_eq!((image.width(), image.height()), (10, 15));
    }

    #[test]
    fn a_webp_is_decoded_as_stored_without_reading_its_exif() {
        let mut webp = Vec::new();
        let mut encoder = WebPEncoder::new_lossless(&mut webp);
        encoder.set_exif_metadata(TURNED_BY_90.to_vec()).unwrap();
        DynamicImage::new_rgb8(3, 2)
            .write_with_encoder(encoder)
            .unwrap();
        let image = decode_unscaled(&webp).unwrap();
        assert_eq!((image.width(), image.height()), (3, 2));

        // An `EXIF` chunk that claims almost 4 GiB, which image-webp would
        // allocate before finding the file too short. Nothing here can see
        // that allocation; the comment in `decode` guards it.
        let exif = webp.windows(4).position(|tag| tag == b"EXIF").unwrap();
        webp[exif + 4..exif + 8].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        let image = decode_unscaled(&webp).unwrap();
        assert_eq!((image.width(), image.height()), (3, 2));
    }
}

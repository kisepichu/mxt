//! Files and images (design, "Files and images"): downloads into
//! `files.download_dir`, uploads, and the image of the message view. Media
//! bytes are never logged, and file names only at `debug`.

use std::{
    fs::{self, OpenOptions},
    io::{self, Cursor, Write as _},
    path::{Path, PathBuf},
};

use image::ImageReader;
use matrix_sdk::{
    Client, Room,
    attachment::{
        AttachmentConfig, AttachmentInfo, BaseAudioInfo, BaseFileInfo, BaseImageInfo, BaseVideoInfo,
    },
    media::{MediaFormat, MediaRequestParameters},
    ruma::{
        UInt,
        events::room::{
            MediaSource,
            message::{ImageMessageEventContent, MessageType},
        },
    },
};
use matrix_sdk_ui::Timeline;
use mime_guess::{Mime, mime};
use mxt_core::{msg::Msg, timeline::ItemRef};
use tracing::debug;

use crate::{
    Error,
    error::{timeline_error, without_url},
    timeline::{enable_sending, message_type},
};

/// The largest image the message view fetches (design, "Images"): 10 MB, in
/// the binary units the timeline shows sizes in.
const PREVIEW_LIMIT: u64 = 10 * 1024 * 1024;

/// `Msg::ImageUnavailable`'s reason for an image above `PREVIEW_LIMIT`.
const TOO_LARGE: &str = "too large to preview";

/// The file name of a download whose event names none that is usable, and
/// of an upload whose path has none.
const FALLBACK_NAME: &str = "download";

/// The longest file name Linux and macOS create, in bytes.
const NAME_MAX: usize = 255;

/// The longest extension `numbered` keeps apart from the stem, in bytes with
/// its dot: in a longer one, the last dot is part of the name (of a
/// sentence used as the name, for example).
const MAX_EXTENSION: usize = 16;

/// The most of an upload that is read for an image's dimensions. The
/// decoders stop after the header, but the JPEG decoder copies all it is
/// given first, and a JPEG's frame header comes after its metadata segments
/// (Exif, ICC profile, XMP), which stay far below this.
const HEADER_LIMIT: usize = 1024 * 1024;

/// `MatrixEffect::Download`: the item's file, decrypted in encrypted rooms,
/// saved in `dir` under the event's file name made safe (see
/// `safe_file_name`) without replacing a file (see `save`); answered with
/// `Msg::Downloaded`.
pub(crate) async fn download(
    client: &Client,
    timeline: &Timeline,
    item: &ItemRef,
    dir: PathBuf,
) -> Result<Msg, Error> {
    let message = message_type(timeline, item).await?;
    let (source, name) = message
        .as_ref()
        .and_then(attachment)
        .ok_or(Error::NoAttachment)?;
    let name = safe_file_name(name);
    let bytes = fetch(client, source).await?;
    let saved = {
        let dir = dir.clone();
        blocking(move || save(&dir, &name, &bytes)).await
    };
    let path = saved.map_err(|error| Error::Io { path: dir, error })?;
    debug!("saved a download as {}", path.display());
    Ok(Msg::Downloaded {
        path: path.to_string_lossy().into_owned(),
    })
}

/// `MatrixEffect::FetchImage` (design, "Images"): the image's thumbnail
/// when the event has one, otherwise the file, up to `PREVIEW_LIMIT`;
/// answered with `Msg::ImageFetched`, or with `Msg::ImageUnavailable` and
/// the reason.
pub(crate) async fn fetch_image(
    client: &Client,
    timeline: &Timeline,
    item: ItemRef,
) -> Result<Msg, Error> {
    let unavailable = |item, reason: &str| Msg::ImageUnavailable {
        item,
        reason: reason.to_owned(),
    };
    let Some(MessageType::Image(image)) = message_type(timeline, &item).await? else {
        return Ok(unavailable(item, "the message is not an image"));
    };
    let Some(source) = preview_source(&image) else {
        return Ok(unavailable(item, TOO_LARGE));
    };
    let bytes = fetch(client, source).await?;
    // The event may give no size, or a wrong one.
    if !fits_preview(bytes.len() as u64) {
        return Ok(unavailable(item, TOO_LARGE));
    }
    Ok(Msg::ImageFetched {
        item,
        bytes: bytes.into(),
    })
}

/// `MatrixEffect::Upload` (design, "Files and images"): the file at `path`
/// (see `expand_home`), queued in `room`'s send queue under its file name.
/// The SDK picks the message type from the MIME type guessed from the name:
/// an image for `image/*`, a video or audio message for `video/*` and
/// `audio/*` (which mxt shows as files), a file otherwise.
///
/// Like a message, it shows as a local echo while it uploads, keeps its
/// place in the room's queue (later messages wait for it), and when it
/// fails stays with its reason for `message.resend` (see `send_queue`). The
/// SDK copies the file into its media store, where it stays until it is
/// sent. Only a regular file up to the homeserver's upload limit is read:
/// a larger one would be read into memory only to be refused.
pub(crate) async fn upload(room: &Room, path: PathBuf) -> Result<(), Error> {
    let io_error = |error| Error::Io {
        path: path.clone(),
        error,
    };
    let metadata = {
        let path = path.clone();
        blocking(move || fs::metadata(path)).await
    };
    let metadata = metadata.map_err(io_error)?;
    if !metadata.is_file() {
        return Err(Error::NotAFile);
    }
    let limit = room
        .client()
        .load_or_fetch_max_upload_size()
        .await
        .map_err(|error| Error::from(without_url(error)))?;
    if metadata.len() > u64::from(limit) {
        return Err(Error::UploadTooLarge {
            limit: limit.into(),
        });
    }
    let read = {
        let path = path.clone();
        blocking(move || fs::read(path)).await
    };
    let bytes = read.map_err(io_error)?;
    let name = path.file_name().map_or(FALLBACK_NAME.into(), |name| {
        name.to_string_lossy().into_owned()
    });
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let config = AttachmentConfig::new().info(attachment_info(&mime, &bytes));
    debug!("uploading {}", path.display());
    enable_sending(room);
    room.send_queue()
        .send_attachment(name, mime, bytes, config)
        .await
        .map(drop)
        .map_err(|error| timeline_error(error.into()))
}

/// The source and the file name of the message's attachment; video and
/// audio are files.
fn attachment(message: &MessageType) -> Option<(MediaSource, &str)> {
    match message {
        MessageType::Image(image) => Some((image.source.clone(), image.filename())),
        MessageType::File(file) => Some((file.source.clone(), file.filename())),
        MessageType::Video(video) => Some((video.source.clone(), video.filename())),
        MessageType::Audio(audio) => Some((audio.source.clone(), audio.filename())),
        _ => None,
    }
}

/// The media's content, decrypted when it is encrypted; from the SDK's media
/// cache when it is there, and kept there.
async fn fetch(client: &Client, source: MediaSource) -> Result<Vec<u8>, Error> {
    let request = MediaRequestParameters {
        source,
        format: MediaFormat::File,
    };
    client
        .media()
        .get_media_content(&request, true)
        .await
        .map_err(|error| without_url(error).into())
}

/// What the message view shows of `image`: the thumbnail when the event has
/// one, otherwise the file; `None` when the event gives it a size above
/// `PREVIEW_LIMIT`.
fn preview_source(image: &ImageMessageEventContent) -> Option<MediaSource> {
    let info = image.info.as_deref();
    let thumbnail = info.and_then(|info| {
        let size = info.thumbnail_info.as_deref().and_then(|thumb| thumb.size);
        Some((info.thumbnail_source.clone()?, size))
    });
    let (source, size) =
        thumbnail.unwrap_or_else(|| (image.source.clone(), info.and_then(|info| info.size)));
    size.is_none_or(|size| fits_preview(size.into()))
        .then_some(source)
}

fn fits_preview(size: u64) -> bool {
    size <= PREVIEW_LIMIT
}

/// The upload's size, and an image's dimensions, in the info of the message
/// type the SDK picks from `mime`: it drops info of another kind.
fn attachment_info(mime: &Mime, bytes: &[u8]) -> AttachmentInfo {
    let size = UInt::new(bytes.len() as u64);
    match mime.type_() {
        mime::IMAGE => {
            let (width, height) = image_dimensions(bytes).unzip();
            AttachmentInfo::Image(BaseImageInfo {
                width,
                height,
                size,
                ..BaseImageInfo::default()
            })
        }
        mime::VIDEO => AttachmentInfo::Video(BaseVideoInfo {
            size,
            ..BaseVideoInfo::default()
        }),
        mime::AUDIO => AttachmentInfo::Audio(BaseAudioInfo {
            size,
            ..BaseAudioInfo::default()
        }),
        _ => AttachmentInfo::File(BaseFileInfo { size }),
    }
}

/// The width and height in the header of the image `bytes`, in the formats
/// the message view decodes, guessed from the bytes rather than the name;
/// `None` when they cannot be read (another format, a damaged file), and
/// the image is sent without them. Nothing past the header is decoded.
fn image_dimensions(bytes: &[u8]) -> Option<(UInt, UInt)> {
    let header = &bytes[..bytes.len().min(HEADER_LIMIT)];
    let (width, height) = ImageReader::new(Cursor::new(header))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    Some((width.into(), height.into()))
}

/// A leading `~/` stands for `home`, as in the configuration; other paths
/// stay as typed, so a relative one is relative to the directory mxt was
/// started in.
pub(crate) fn expand_home(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(path),
    }
}

/// A file name from an event, made safe to create in the download
/// directory: it names a file directly in it, and not a hidden one. Path
/// separators, control characters and bidirectional formatting characters
/// (Unicode's `Bidi_Control`, which could make "Saved <path>" show another
/// extension than the one the opener goes by) become `_`, leading dots are
/// dropped, and a name that is empty then becomes `download`.
fn safe_file_name(name: &str) -> String {
    let safe: String = name
        .trim_start_matches('.')
        .chars()
        .map(|c| {
            let bidi = matches!(
                c,
                '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            );
            if matches!(c, '/' | '\\') || c.is_control() || bidi {
                '_'
            } else {
                c
            }
        })
        .collect();
    if safe.is_empty() {
        FALLBACK_NAME.to_owned()
    } else {
        safe
    }
}

/// The `n`th name to try for `name` (design, "Files and images"): `name`
/// itself, then `name (1).ext`, `name (2).ext`, …, where the extension
/// starts at the last dot and is at most `MAX_EXTENSION` long. The stem is
/// cut at a character boundary so that the name fits in `NAME_MAX`: the
/// sender picks the name (a caption, with older clients), and a longer one
/// could not be saved at all.
fn numbered(name: &str, n: u32) -> String {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= MAX_EXTENSION => name.split_at(dot),
        _ => (name, ""),
    };
    let number = if n == 0 {
        String::new()
    } else {
        format!(" ({n})")
    };
    let stem = &stem[..stem.floor_char_boundary(NAME_MAX - number.len() - extension.len())];
    format!("{stem}{number}{extension}")
}

/// Saves `bytes` in `dir`, which is created when it is missing, under the
/// first name of `numbered` that is free. It never replaces a file, not even
/// one created meanwhile (`create_new`, which does not follow symbolic
/// links either). A file that cannot be written completely is removed.
fn save(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    for n in 0..=u32::MAX {
        let path = dir.join(numbered(name, n));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        return Ok(path);
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

/// `f` on the blocking pool: file system calls would hold up the runtime's
/// workers. A panic in `f` is passed on.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|error| match error.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            // Cancelled: the runtime is shutting down.
            Err(error) => Err(io::Error::other(error)),
        })
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::{
        OwnedMxcUri,
        events::room::{
            ImageInfo, ThumbnailInfo,
            message::{AudioInfo, FileInfo, VideoInfo},
        },
        owned_mxc_uri, uint,
    };

    use super::*;
    use crate::test_dir::TestDir;

    #[test]
    fn the_first_name_is_the_events_and_later_ones_are_numbered_before_the_extension() {
        assert_eq!(numbered("cat.png", 0), "cat.png");
        assert_eq!(numbered("cat.png", 1), "cat (1).png");
        assert_eq!(numbered("cat.png", 2), "cat (2).png");
        assert_eq!(numbered("archive.tar.gz", 1), "archive.tar (1).gz");
        assert_eq!(numbered("README", 3), "README (3)");
    }

    #[test]
    fn long_names_are_cut_to_255_bytes_at_a_character_and_keep_their_extension() {
        let ascii = format!("{}.png", "a".repeat(296));
        assert_eq!(ascii.len(), 300);
        assert_eq!(numbered(&ascii, 0), format!("{}.png", "a".repeat(251)));
        assert_eq!(
            numbered(&ascii, 12),
            format!("{} (12).png", "a".repeat(246))
        );
        // 3 bytes a character: 83 of them fit before the extension, 82
        // before the number too.
        let cjk = format!("{}.pdf", "資料".repeat(50));
        let first = numbered(&cjk, 0);
        assert_eq!(first, format!("{}資.pdf", "資料".repeat(41)));
        assert_eq!(first.len(), 253);
        let twelfth = numbered(&cjk, 12);
        assert_eq!(twelfth, format!("{} (12).pdf", "資料".repeat(41)));
        assert_eq!(twelfth.len(), 255);
    }

    #[test]
    fn an_extension_above_sixteen_bytes_is_part_of_the_name() {
        assert_eq!(numbered("x.abcdefghijklmno", 1), "x (1).abcdefghijklmno");
        assert_eq!(
            numbered("see you. at the north exit", 1),
            "see you. at the north exit (1)"
        );
        let long = format!("{}.{}", "a".repeat(200), "b".repeat(100));
        assert_eq!(numbered(&long, 1), format!("{} (1)", &long[..251]));
    }

    #[test]
    fn safe_file_names_stay_in_the_directory_and_are_not_hidden() {
        assert_eq!(safe_file_name("cat.png"), "cat.png");
        assert_eq!(
            safe_file_name("../../.ssh/authorized_keys"),
            "_.._.ssh_authorized_keys"
        );
        assert_eq!(safe_file_name("/etc/passwd"), "_etc_passwd");
        assert_eq!(safe_file_name("a\\b.txt"), "a_b.txt");
        assert_eq!(safe_file_name(".bashrc"), "bashrc");
        assert_eq!(safe_file_name("two\nlines\0.txt"), "two_lines_.txt");
        // A right-to-left override would show `invoice` and `hs.pdf`.
        assert_eq!(safe_file_name("invoice\u{202E}fdp.sh"), "invoice_fdp.sh");
        assert_eq!(
            safe_file_name("a\u{200E}\u{200F}\u{202A}\u{202C}\u{2066}\u{2069}\u{061C}b.txt"),
            "a_______b.txt"
        );
        assert_eq!(safe_file_name("資料.pdf"), "資料.pdf");
        assert_eq!(safe_file_name(".."), "download");
        assert_eq!(safe_file_name(""), "download");
    }

    #[test]
    fn a_download_never_replaces_a_file() {
        let dir = TestDir::new("download-collision");
        fs::write(dir.join("cat.png"), "first").unwrap();
        fs::write(dir.join("cat (1).png"), "second").unwrap();

        let saved = save(&dir, "cat.png", b"third").unwrap();

        assert_eq!(saved, dir.join("cat (2).png"));
        assert_eq!(fs::read(&saved).unwrap(), b"third");
        assert_eq!(fs::read(dir.join("cat.png")).unwrap(), b"first");
        assert_eq!(fs::read(dir.join("cat (1).png")).unwrap(), b"second");
    }

    #[test]
    fn a_missing_download_directory_is_created() {
        let dir = TestDir::new("download-missing-dir");
        let downloads = dir.join("Downloads");

        let saved = save(&downloads, "cat.png", b"cat").unwrap();

        assert_eq!(saved, downloads.join("cat.png"));
        assert_eq!(fs::read(&saved).unwrap(), b"cat");
    }

    #[test]
    fn a_long_name_is_numbered_when_its_shortened_name_exists() {
        let dir = TestDir::new("download-long-name");
        let name = format!("{}.png", "a".repeat(296));
        fs::write(dir.join(numbered(&name, 0)), "first").unwrap();

        let saved = save(&dir, &name, b"second").unwrap();

        assert_eq!(saved, dir.join(numbered(&name, 1)));
        assert_eq!(fs::read(&saved).unwrap(), b"second");
        assert_eq!(fs::read(dir.join(numbered(&name, 0))).unwrap(), b"first");
    }

    fn image(size: Option<u64>, thumbnail: Option<Option<u64>>) -> ImageMessageEventContent {
        let mut info = ImageInfo::new();
        info.size = size.and_then(UInt::new);
        if let Some(thumbnail_size) = thumbnail {
            let mut thumbnail_info = ThumbnailInfo::new();
            thumbnail_info.size = thumbnail_size.and_then(UInt::new);
            info.thumbnail_source = Some(MediaSource::Plain(owned_mxc_uri!(
                "mxc://example.org/thumbnail"
            )));
            info.thumbnail_info = Some(Box::new(thumbnail_info));
        }
        let mut image = ImageMessageEventContent::plain(
            "cat.png".to_owned(),
            owned_mxc_uri!("mxc://example.org/file"),
        );
        image.info = Some(Box::new(info));
        image
    }

    fn preview_uri(image: &ImageMessageEventContent) -> Option<OwnedMxcUri> {
        match preview_source(image)? {
            MediaSource::Plain(uri) => Some(uri),
            MediaSource::Encrypted(_) => unreachable!("the test images are not encrypted"),
        }
    }

    #[test]
    fn previews_fetch_the_thumbnail_when_the_event_has_one() {
        let thumbnail = Some(owned_mxc_uri!("mxc://example.org/thumbnail"));
        assert_eq!(preview_uri(&image(Some(1024), Some(Some(64)))), thumbnail);
        // The thumbnail is fetched also for a file above the limit.
        let large = Some(PREVIEW_LIMIT * 2);
        assert_eq!(preview_uri(&image(large, Some(None))), thumbnail);
        let file = Some(owned_mxc_uri!("mxc://example.org/file"));
        assert_eq!(preview_uri(&image(Some(1024), None)), file);
    }

    #[test]
    fn previews_above_ten_megabytes_are_not_fetched() {
        let file = Some(owned_mxc_uri!("mxc://example.org/file"));
        assert_eq!(preview_uri(&image(Some(PREVIEW_LIMIT), None)), file);
        assert_eq!(preview_uri(&image(Some(PREVIEW_LIMIT + 1), None)), None);
        let large_thumbnail = Some(Some(PREVIEW_LIMIT + 1));
        assert_eq!(preview_uri(&image(Some(1024), large_thumbnail)), None);
        // Without a size, the fetched bytes are checked.
        assert_eq!(preview_uri(&image(None, None)), file);
        assert!(fits_preview(PREVIEW_LIMIT));
        assert!(!fits_preview(PREVIEW_LIMIT + 1));
    }

    #[test]
    fn uploads_keep_their_size_in_the_message_type_the_sdk_picks() {
        let size = Some(uint!(5));
        let info = |mime: &str| attachment_info(&mime.parse().unwrap(), b"bytes");
        assert_eq!(ImageInfo::from(info("image/png")).size, size);
        assert_eq!(VideoInfo::from(info("video/mp4")).size, size);
        assert_eq!(AudioInfo::from(info("audio/ogg")).size, size);
        assert_eq!(FileInfo::from(info("application/pdf")).size, size);
    }

    /// The signature and the header chunk of a 640x480 PNG, then the start
    /// of its image data, which the decoder reads up to.
    const PNG_HEADER: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, // signature
        0, 0, 0, 13, b'I', b'H', b'D', b'R', // IHDR
        0, 0, 0x02, 0x80, 0, 0, 0x01, 0xE0, 8, 2, 0, 0, 0, // 640x480, 8-bit RGB
        0xBA, 0xB3, 0x4B, 0xB3, // CRC
        0, 0, 0, 0, b'I', b'D', b'A', b'T', // IDAT
    ];

    /// The start of a 1920x1080 grayscale JPEG: the frame header, then the
    /// scan header, which the decoder reads up to.
    const JPEG_HEADER: &[u8] = &[
        0xFF, 0xD8, // SOI
        0xFF, 0xC0, 0, 11, 8, // SOF0, 8-bit samples
        0x04, 0x38, 0x07, 0x80, // 1080 lines, 1920 columns
        1, 1, 0x11, 0, // one component
        0xFF, 0xDA, 0, 8, 1, 1, 0, 0, 0x3F, 0, // SOS
    ];

    fn image_info(mime: &str, bytes: &[u8]) -> ImageInfo {
        ImageInfo::from(attachment_info(&mime.parse().unwrap(), bytes))
    }

    #[test]
    fn uploaded_images_carry_the_dimensions_in_their_header() {
        let png = image_info("image/png", PNG_HEADER);
        assert_eq!(
            (png.width, png.height),
            (Some(uint!(640)), Some(uint!(480)))
        );
        let jpeg = image_info("image/jpeg", JPEG_HEADER);
        assert_eq!(
            (jpeg.width, jpeg.height),
            (Some(uint!(1920)), Some(uint!(1080)))
        );
        // The format comes from the bytes, not from the file name.
        let misnamed = image_info("image/jpeg", PNG_HEADER);
        assert_eq!(misnamed.width, Some(uint!(640)));
    }

    #[test]
    fn an_image_whose_header_cannot_be_read_is_sent_without_dimensions() {
        for bytes in [&b"<svg></svg>"[..], &PNG_HEADER[..20]] {
            let info = image_info("image/png", bytes);
            assert_eq!((info.width, info.height), (None, None));
            assert_eq!(info.size, UInt::new(bytes.len() as u64));
        }
    }

    #[test]
    fn upload_paths_expand_a_leading_tilde_slash() {
        let expanded = |path| expand_home(path, Path::new("/home/alice"));
        assert_eq!(expanded("~/cat.png"), Path::new("/home/alice/cat.png"));
        assert_eq!(expanded("~//cat.png"), Path::new("/home/alice/cat.png"));
        assert_eq!(expanded("cat.png"), Path::new("cat.png"));
        assert_eq!(expanded("/tmp/cat.png"), Path::new("/tmp/cat.png"));
        assert_eq!(expanded("~bob/cat.png"), Path::new("~bob/cat.png"));
    }
}

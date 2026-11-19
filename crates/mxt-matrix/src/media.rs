//! Files and images (design, "Files and images"): downloads into
//! `files.download_dir`, uploads, and the image of the message view. Media
//! bytes are never logged, and file names only at `debug`.

use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

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

use crate::{Error, error::without_url, timeline::message_type};

/// The largest image the message view fetches (design, "Images"): 10 MB, in
/// the binary units the timeline shows sizes in.
const PREVIEW_LIMIT: u64 = 10 * 1024 * 1024;

/// `Msg::ImageUnavailable`'s reason for an image above `PREVIEW_LIMIT`.
const TOO_LARGE: &str = "too large to preview";

/// The file name of a download whose event names none that is usable, and
/// of an upload whose path has none.
const FALLBACK_NAME: &str = "download";

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

/// `MatrixEffect::Upload` (design, "Files and images"): the file at `path`,
/// where a leading `~/` stands for `home` and a relative path is relative to
/// the directory mxt was started in, sent to `room` under its file name. The
/// SDK picks the message type from the MIME type guessed from the name: an
/// image for `image/*`, a video or audio message for `video/*` and
/// `audio/*` (which mxt shows as files), a file otherwise.
pub(crate) async fn upload(room: &Room, path: &str, home: Option<&OsStr>) -> Result<(), Error> {
    let path = expand_home(path, home);
    let read = {
        let path = path.clone();
        blocking(move || fs::read(path)).await
    };
    let bytes = read.map_err(|error| Error::Io {
        path: path.clone(),
        error,
    })?;
    let name = path.file_name().map_or(FALLBACK_NAME.into(), |name| {
        name.to_string_lossy().into_owned()
    });
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let config = AttachmentConfig::new().info(attachment_info(&mime, bytes.len()));
    debug!("uploading {}", path.display());
    room.send_attachment(name, &mime, bytes, config)
        .store_in_cache()
        .await
        .map(drop)
        .map_err(|error| without_url(error).into())
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

/// The upload's size, in the info of the message type the SDK picks from
/// `mime`: it drops info of another kind.
fn attachment_info(mime: &Mime, len: usize) -> AttachmentInfo {
    let size = UInt::new(len as u64);
    match mime.type_() {
        mime::IMAGE => AttachmentInfo::Image(BaseImageInfo {
            size,
            ..BaseImageInfo::default()
        }),
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

/// A leading `~/` stands for `home`, as in the configuration; other paths
/// stay as typed.
fn expand_home(path: &str, home: Option<&OsStr>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => Path::new(home).join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

/// A file name from an event, made safe to create in the download
/// directory: it names a file directly in it, and not a hidden one. Path
/// separators and control characters become `_`, leading dots are dropped,
/// and a name that is empty then becomes `download`.
fn safe_file_name(name: &str) -> String {
    let safe: String = name
        .trim_start_matches('.')
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\') || c.is_control() {
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
/// starts at the last dot.
fn numbered(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    format!("{stem} ({n}){extension}")
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
        let info = |mime: &str| attachment_info(&mime.parse().unwrap(), 5);
        assert_eq!(ImageInfo::from(info("image/png")).size, size);
        assert_eq!(VideoInfo::from(info("video/mp4")).size, size);
        assert_eq!(AudioInfo::from(info("audio/ogg")).size, size);
        assert_eq!(FileInfo::from(info("application/pdf")).size, size);
    }

    #[test]
    fn upload_paths_expand_a_leading_tilde_slash() {
        let home = Some(OsStr::new("/home/alice"));
        let expanded = |path| expand_home(path, home);
        assert_eq!(expanded("~/cat.png"), Path::new("/home/alice/cat.png"));
        assert_eq!(expanded("~//cat.png"), Path::new("/home/alice/cat.png"));
        assert_eq!(expanded("cat.png"), Path::new("cat.png"));
        assert_eq!(expanded("/tmp/cat.png"), Path::new("/tmp/cat.png"));
        assert_eq!(expanded("~bob/cat.png"), Path::new("~bob/cat.png"));
        assert_eq!(expand_home("~/cat.png", None), Path::new("~/cat.png"));
    }
}

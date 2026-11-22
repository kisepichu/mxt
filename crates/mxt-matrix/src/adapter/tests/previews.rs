//! `FetchPreview` (design, "Images in the timeline"): the event's thumbnail
//! first, plain or encrypted, then the server's thumbnail of a plain file
//! and the file itself, the 10 MiB limit, stickers, and `PreviewFailed` for
//! every failure.

use std::io::Cursor;

use matrix_sdk::ruma::{
    OwnedMxcUri, UInt,
    events::{
        AnySyncTimelineEvent,
        room::{
            EncryptedFile, ImageInfo, MediaSource, ThumbnailInfo,
            message::{ImageMessageEventContent, MessageType, RoomMessageEventContent},
        },
    },
    media::Method,
    serde::Raw,
};
use serde_json::json;
use wiremock::ResponseTemplate;

use super::*;

/// The authenticated media endpoints the SDK uses with Matrix 1.12.
const DOWNLOAD: &str = "/_matrix/client/v1/media/download/";
const THUMBNAIL: &str = "/_matrix/client/v1/media/thumbnail/";
/// 11 MiB, above the 10 MiB limit.
const TOO_LARGE: u64 = 11 * 1024 * 1024;

fn mxc(media: &str) -> OwnedMxcUri {
    OwnedMxcUri::from(format!("mxc://example.org/{media}"))
}

fn plain(media: &str) -> MediaSource {
    MediaSource::Plain(mxc(media))
}

/// An image's info with `size`, and `thumbnail` as its thumbnail's source.
fn info(size: Option<u64>, thumbnail: Option<MediaSource>) -> ImageInfo {
    let mut info = ImageInfo::new();
    info.width = Some(UInt::from(640_u32));
    info.height = Some(UInt::from(480_u32));
    info.size = size.and_then(UInt::new);
    if let Some(thumbnail) = thumbnail {
        info.thumbnail_source = Some(thumbnail);
        info.thumbnail_info = Some(Box::new(ThumbnailInfo::new()));
    }
    info
}

/// The request paths that name `media`.
fn naming(requests: &[Request], media: &str) -> usize {
    let suffix = format!("/example.org/{media}");
    requests
        .iter()
        .filter(|request| request.url.path().ends_with(&suffix))
        .count()
}

impl Adapter {
    /// `event`, built by the factory, synced into `ROOM` as `$<name>`, once
    /// the timeline has it.
    async fn sync_event(
        &mut self,
        name: &str,
        event: impl Into<Raw<AnySyncTimelineEvent>>,
    ) -> ItemRef {
        let room_id = ruma::RoomId::parse(ROOM).unwrap();
        let joined = JoinedRoomBuilder::new(&room_id).add_timeline_event(event);
        self.server.sync_room(&self.client, joined).await;
        let item = ItemRef::Remote(EventId::new(format!("${name}:example.org")));
        self.core
            .until("the event", |entries| super::item(entries, &item).is_some())
            .await;
        item
    }

    /// An image message with `source` and `info`, synced into `ROOM`.
    async fn image(&mut self, name: &str, source: MediaSource, info: ImageInfo) -> ItemRef {
        let me = ruma::UserId::parse(ME).unwrap();
        let event = ruma::EventId::parse(format!("${name}:example.org")).unwrap();
        let mut image = ImageMessageEventContent::new("photo.png".to_owned(), source);
        image.info = Some(Box::new(info));
        let content = RoomMessageEventContent::new(MessageType::Image(image));
        let built = EventFactory::new()
            .event(content)
            .sender(&me)
            .event_id(&event);
        self.sync_event(name, built).await
    }

    /// `bytes` encrypted and uploaded as `media`: the file's encryption info
    /// and the ciphertext the server keeps. The upload asks for the
    /// homeserver's upload limit first.
    async fn encrypted(&self, media: &str, bytes: &[u8]) -> (EncryptedFile, Vec<u8>) {
        let server = &self.server;
        server
            .mock_authenticated_media_config()
            .ok_default()
            .mount()
            .await;
        let (uploaded, mock) = server.mock_upload().ok_with_capture(&mxc(media));
        mock.mock_once().mount().await;
        let file = self
            .client
            .upload_encrypted_file(&mut Cursor::new(bytes))
            .await
            .unwrap();
        (file, uploaded.await.unwrap())
    }

    /// The answer to `FetchPreview` for `item` in `ROOM`: its bytes, or
    /// `None` for `PreviewFailed`.
    async fn preview(&mut self, item: &ItemRef) -> Option<Vec<u8>> {
        self.effect(MatrixEffect::FetchPreview {
            room: room(),
            item: item.clone(),
        });
        match self.core.report().await {
            Msg::PreviewFetched {
                room: answered,
                item: fetched,
                bytes,
            } => {
                assert_eq!((&answered, &fetched), (&room(), item));
                Some(bytes.as_bytes().to_vec())
            }
            Msg::PreviewFailed {
                room: answered,
                item: failed,
            } => {
                assert_eq!((&answered, &failed), (&room(), item));
                None
            }
            other => panic!("not the answer of the preview: {other:?}"),
        }
    }

    /// The server's 640 by 480 thumbnail, scaled and not animated, answered
    /// with `status`, expected once.
    async fn mock_thumbnail(&self, status: u16) {
        let thumbnail = self
            .server
            .mock_authed_media_thumbnail(Method::Scale, 640, 480, false);
        let mock = match status {
            200 => thumbnail.ok(),
            _ => thumbnail.respond_with(ResponseTemplate::new(status).set_body_json(json!({
                "errcode": "M_NOT_FOUND",
                "error": "Not found",
            }))),
        };
        mock.expect(1).mount().await;
    }
}

#[tokio::test]
async fn an_image_with_a_plain_thumbnail_fetches_the_thumbnail_and_not_the_file() {
    let mut adapter = Adapter::start().await;
    adapter
        .server
        .mock_authed_media_download()
        .ok_bytes(b"thumbnail".to_vec())
        .expect(1)
        .mount()
        .await;
    let thumbnail = Some(plain("thumbnail"));
    let image = adapter
        .image("photo", plain("photo"), info(Some(4096), thumbnail))
        .await;

    assert_eq!(
        adapter.preview(&image).await.as_deref(),
        Some(&b"thumbnail"[..])
    );
    let downloads = adapter.requests(DOWNLOAD).await;
    assert_eq!(naming(&downloads, "thumbnail"), 1, "{downloads:?}");
    assert!(adapter.requests(THUMBNAIL).await.is_empty());
}

#[tokio::test]
async fn an_image_with_an_encrypted_thumbnail_decrypts_the_thumbnail() {
    let mut adapter = Adapter::start().await;
    let (file, _) = adapter.encrypted("photo", b"the photo").await;
    let (thumbnail, ciphertext) = adapter.encrypted("thumbnail", b"the thumbnail").await;
    adapter
        .server
        .mock_authed_media_download()
        .ok_bytes(ciphertext)
        .expect(1)
        .mount()
        .await;
    let source = MediaSource::Encrypted(Box::new(file));
    let thumbnail = Some(MediaSource::Encrypted(Box::new(thumbnail)));
    let image = adapter
        .image("photo", source, info(Some(4096), thumbnail))
        .await;

    assert_eq!(
        adapter.preview(&image).await.as_deref(),
        Some(&b"the thumbnail"[..])
    );
    assert_eq!(naming(&adapter.requests(DOWNLOAD).await, "thumbnail"), 1);
}

#[tokio::test]
async fn a_plain_image_without_a_thumbnail_asks_the_server_for_one() {
    let mut adapter = Adapter::start().await;
    adapter.mock_thumbnail(200).await;
    adapter
        .server
        .mock_authed_media_download()
        .ok_image()
        .expect(0)
        .mount()
        .await;
    let image = adapter
        .image("photo", plain("photo"), info(Some(4096), None))
        .await;

    assert_eq!(
        adapter.preview(&image).await.as_deref(),
        Some(&b"binaryjpegthumbnaildata"[..])
    );
    assert_eq!(naming(&adapter.requests(THUMBNAIL).await, "photo"), 1);
}

#[tokio::test]
async fn without_the_server_s_thumbnail_a_plain_image_fetches_the_file() {
    let mut adapter = Adapter::start().await;
    adapter.mock_thumbnail(404).await;
    adapter
        .server
        .mock_authed_media_download()
        .ok_bytes(b"the photo".to_vec())
        .expect(1)
        .mount()
        .await;
    let image = adapter
        .image("photo", plain("photo"), info(Some(4096), None))
        .await;

    assert_eq!(
        adapter.preview(&image).await.as_deref(),
        Some(&b"the photo"[..])
    );
    assert_eq!(naming(&adapter.requests(DOWNLOAD).await, "photo"), 1);
}

#[tokio::test]
async fn an_encrypted_image_without_a_thumbnail_decrypts_the_file() {
    let mut adapter = Adapter::start().await;
    let (file, ciphertext) = adapter.encrypted("photo", b"the photo").await;
    adapter
        .server
        .mock_authed_media_download()
        .ok_bytes(ciphertext)
        .expect(1)
        .mount()
        .await;
    let source = MediaSource::Encrypted(Box::new(file));
    let image = adapter.image("photo", source, info(Some(4096), None)).await;

    assert_eq!(
        adapter.preview(&image).await.as_deref(),
        Some(&b"the photo"[..])
    );
    assert!(adapter.requests(THUMBNAIL).await.is_empty());
}

#[tokio::test]
async fn an_encrypted_image_above_ten_mib_without_a_thumbnail_is_not_fetched() {
    let mut adapter = Adapter::start().await;
    let (file, _) = adapter.encrypted("photo", b"the photo").await;
    let source = MediaSource::Encrypted(Box::new(file));
    let image = adapter
        .image("photo", source, info(Some(TOO_LARGE), None))
        .await;

    assert_eq!(adapter.preview(&image).await, None);
    assert!(adapter.requests(DOWNLOAD).await.is_empty());
    assert!(adapter.requests(THUMBNAIL).await.is_empty());
}

#[tokio::test]
async fn a_plain_image_above_ten_mib_without_a_thumbnail_asks_only_for_the_server_s() {
    let mut adapter = Adapter::start().await;
    adapter.mock_thumbnail(404).await;
    adapter
        .server
        .mock_authed_media_download()
        .ok_image()
        .expect(0)
        .mount()
        .await;
    let image = adapter
        .image("photo", plain("photo"), info(Some(TOO_LARGE), None))
        .await;

    assert_eq!(adapter.preview(&image).await, None);
    assert_eq!(adapter.requests(THUMBNAIL).await.len(), 1);
    assert!(adapter.requests(DOWNLOAD).await.is_empty());
}

#[tokio::test]
async fn a_sticker_s_source_is_fetched() {
    let mut adapter = Adapter::start().await;
    adapter.mock_thumbnail(200).await;
    let me = ruma::UserId::parse(ME).unwrap();
    let event = ruma::EventId::parse("$sticker:example.org").unwrap();
    let mut info = ImageInfo::new();
    info.width = Some(UInt::from(256_u32));
    info.height = Some(UInt::from(256_u32));
    let built = EventFactory::new()
        .sticker("a happy cat", info, mxc("sticker"))
        .sender(&me)
        .event_id(&event);
    let sticker = adapter.sync_event("sticker", built).await;
    let content = item(&adapter.core.entries, &sticker).map(|item| &item.content);
    assert_eq!(
        content,
        Some(&MessageContent::Sticker {
            body: "a happy cat".to_owned(),
            dimensions: Some((256, 256)),
        })
    );

    assert_eq!(
        adapter.preview(&sticker).await.as_deref(),
        Some(&b"binaryjpegthumbnaildata"[..])
    );
    assert_eq!(naming(&adapter.requests(THUMBNAIL).await, "sticker"), 1);
}

#[tokio::test]
async fn a_text_message_or_a_room_that_is_not_open_answers_preview_failed() {
    let mut adapter = Adapter::start().await;
    assert_eq!(adapter.preview(&sent()).await, None);

    adapter.effect(MatrixEffect::FetchPreview {
        room: RoomId::new("!other:example.org"),
        item: sent(),
    });
    assert!(matches!(
        adapter.core.report().await,
        Msg::PreviewFailed { room, .. } if room == RoomId::new("!other:example.org")
    ));
    assert!(adapter.requests(DOWNLOAD).await.is_empty());
    assert!(adapter.requests(THUMBNAIL).await.is_empty());
}

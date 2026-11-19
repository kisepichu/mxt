//! The send queue paths against a mock homeserver (matrix-sdk's
//! `MatrixMockServer` on 127.0.0.1): the effects run as `Matrix::start` runs
//! them, without the sync loop and the room list, and the tests read what
//! reaches the core.

use std::{collections::VecDeque, fs, time::Duration};

use matrix_sdk::{
    ruma::{
        self, event_id,
        events::secret_storage::key::{
            SecretStorageEncryptionAlgorithm, SecretStorageKeyEventContent,
            SecretStorageV1AesHmacSha2Properties,
        },
        owned_device_id,
        serde::Base64,
    },
    send_queue::{LocalEcho, LocalEchoContent},
    test_utils::mocks::MatrixMockServer,
};
use matrix_sdk_test::{JoinedRoomBuilder, event_factory::EventFactory};
use mxt_core::{
    effect::OutgoingKind,
    mention::RenderedBody,
    timeline::{
        ItemRef, MessageContent, SendState, TimelineDiff, TimelineEntry, TimelineItem, apply_diff,
    },
};
use tokio::time::timeout;

use super::*;
use crate::test_dir::TestDir;

const ROOM: &str = "!room:example.org";
const ME: &str = "@alice:example.org";
/// The user's message in `ROOM`.
const SENT: &str = "$sent:example.org";
/// How long a test waits for each message of the adapter.
const WAIT: Duration = Duration::from_secs(10);

fn room() -> RoomId {
    RoomId::new(ROOM)
}

fn sent() -> ItemRef {
    ItemRef::Remote(EventId::new(SENT))
}

/// The adapter for the user's client of a mock homeserver, with `ROOM`
/// open, which holds the user's message `SENT`.
struct Adapter {
    server: MatrixMockServer,
    client: Client,
    effects: UnboundedSender<MatrixEffect>,
    core: Core,
    _tasks: JoinSet<()>,
}

impl Adapter {
    async fn start() -> Adapter {
        let server = MatrixMockServer::new().await;
        let me = ruma::UserId::parse(ME).unwrap();
        let client = server
            .client_builder()
            // The access token the mocks expect.
            .logged_in_with_token("1234".to_owned(), me.clone(), owned_device_id!("DEVICE"))
            // The mock homeserver speaks plain HTTP. Otherwise building the
            // HTTP client loads the system's CA certificates, which fails on
            // hosts without any.
            .on_builder(|builder| builder.disable_ssl_verification())
            .build()
            .await;
        server.mock_room_state_encryption().plain().mount().await;
        // As `SyncLoop::start` does, so the event cache has the message, as
        // an edit's target.
        client.event_cache().subscribe().unwrap();
        let event = ruma::EventId::parse(SENT).unwrap();
        let message = EventFactory::new()
            .text_msg("hello")
            .sender(&me)
            .event_id(&event);
        let room_id = ruma::RoomId::parse(ROOM).unwrap();
        let joined = JoinedRoomBuilder::new(&room_id).add_timeline_event(message);
        server.sync_room(&client, joined).await;

        let (to_core, from_adapter) = mpsc::unbounded_channel();
        let (effects, queue) = mpsc::unbounded_channel();
        let settings = client.notification_settings().await;
        let files = Files {
            download_dir: PathBuf::new(),
            home: PathBuf::new(),
        };
        let mut tasks = JoinSet::new();
        spawn_send_paths(
            &mut tasks,
            client.clone(),
            settings,
            ReceiptKind::Public,
            files,
            to_core,
            queue,
        );
        let mut adapter = Adapter {
            server,
            client,
            effects,
            core: Core::new(from_adapter),
            _tasks: tasks,
        };
        adapter.effect(MatrixEffect::SubscribeTimeline(room()));
        adapter
            .core
            .until("the timeline", |entries| item(entries, &sent()).is_some())
            .await;
        adapter
    }

    fn effect(&self, effect: MatrixEffect) {
        self.effects.send(effect).unwrap();
    }

    /// The requests in `ROOM`'s send queue.
    async fn queued(&self) -> Vec<LocalEcho> {
        let mut queues = self.client.send_queue().local_echoes().await.unwrap();
        let room = ruma::RoomId::parse(ROOM).unwrap();
        queues.remove(&room).unwrap_or_default()
    }
}

/// What the adapter sends to the core: the open room's entries, built as
/// the core builds them, and the answers of sends and the failures (see
/// `report`).
struct Core {
    from_adapter: UnboundedReceiver<Msg>,
    entries: Vec<TimelineEntry>,
    /// Received and not applied yet: `until` applies one at a time, so it
    /// sees every state of the entries.
    diffs: VecDeque<TimelineDiff>,
    reports: VecDeque<Msg>,
}

impl Core {
    fn new(from_adapter: UnboundedReceiver<Msg>) -> Core {
        Core {
            from_adapter,
            entries: Vec::new(),
            diffs: VecDeque::new(),
            reports: VecDeque::new(),
        }
    }

    /// Applies the timeline's diffs until `done` holds.
    async fn until(&mut self, what: &str, done: impl Fn(&[TimelineEntry]) -> bool) {
        while !done(&self.entries) {
            match self.diffs.pop_front() {
                Some(diff) => apply_diff(&mut self.entries, diff),
                None => self.receive(what).await,
            }
        }
    }

    /// The next answer of a send, of a recovery, or report of a failure
    /// (`SendQueued`, `SendFailed`, `Recovered`, `EffectFailed`); other
    /// messages are skipped.
    async fn report(&mut self) -> Msg {
        loop {
            if let Some(report) = self.reports.pop_front() {
                return report;
            }
            self.receive("a report").await;
        }
    }

    async fn receive(&mut self, what: &str) {
        match timeout(WAIT, self.from_adapter.recv()).await {
            Ok(Some(Msg::Timeline { diffs, .. })) => self.diffs.extend(diffs),
            Ok(Some(
                report @ (Msg::SendQueued { .. }
                | Msg::SendFailed { .. }
                | Msg::Recovered
                | Msg::EffectFailed { .. }),
            )) => self.reports.push_back(report),
            Ok(Some(_)) => {}
            Ok(None) => panic!("the adapter stopped while waiting for {what}"),
            Err(_) => panic!("waited {WAIT:?} for {what}"),
        }
    }
}

fn item<'a>(entries: &'a [TimelineEntry], id: &ItemRef) -> Option<&'a TimelineItem> {
    entries.iter().find_map(|entry| match entry {
        TimelineEntry::Item(item) if item.id == *id => Some(item),
        _ => None,
    })
}

/// The keys of the reactions to `SENT`.
fn reactions(entries: &[TimelineEntry]) -> Vec<&str> {
    item(entries, &sent()).map_or_else(Vec::new, |item| {
        item.reactions
            .iter()
            .map(|group| group.key.as_str())
            .collect()
    })
}

fn edit_of_sent(text: &str) -> Outgoing {
    Outgoing {
        body: RenderedBody {
            plain: text.to_owned(),
            markdown: text.to_owned(),
            user_ids: Vec::new(),
        },
        kind: OutgoingKind::Edit { target: sent() },
    }
}

#[tokio::test]
async fn a_refused_edit_of_a_sent_message_answers_its_send_with_send_failed() {
    let mut adapter = Adapter::start().await;
    adapter
        .server
        .mock_room_send()
        .error_too_large()
        .mock_once()
        .mount()
        .await;

    let id = SendId::new(1);
    let message = edit_of_sent("fixed");
    adapter.effect(MatrixEffect::Send {
        room: room(),
        id,
        message,
    });

    match adapter.core.report().await {
        Msg::SendFailed { id: failed, error } => {
            assert_eq!(failed, id);
            assert!(error.starts_with("Editing the message failed: "), "{error}");
        }
        other => panic!("not the answer of the send: {other:?}"),
    }
    // Dropped, so it does not hold back the room's later requests.
    assert!(adapter.queued().await.is_empty());
}

#[tokio::test]
async fn an_edit_of_a_sent_message_is_answered_with_send_queued_once_sent() {
    let mut adapter = Adapter::start().await;
    adapter
        .server
        .mock_room_send()
        .ok(event_id!("$edit:example.org"))
        .mock_once()
        .mount()
        .await;

    let id = SendId::new(1);
    let message = edit_of_sent("fixed");
    adapter.effect(MatrixEffect::Send {
        room: room(),
        id,
        message,
    });

    assert_eq!(adapter.core.report().await, Msg::SendQueued { id });
    // Answered once it left the send queue, not when it was queued.
    assert!(adapter.queued().await.is_empty());
}

#[tokio::test]
async fn a_refused_upload_stays_as_a_failed_echo_and_is_reported() {
    let mut adapter = Adapter::start().await;
    let server = &adapter.server;
    server
        .mock_authenticated_media_config()
        .ok_default()
        .mount()
        .await;
    server
        .mock_upload()
        .error_too_large()
        .mock_once()
        .mount()
        .await;
    let dir = TestDir::new("refused-upload");
    let path = dir.join("notes.txt");
    fs::write(&path, "notes").unwrap();

    adapter.effect(MatrixEffect::Upload {
        room: room(),
        path: path.to_string_lossy().into_owned(),
    });

    match adapter.core.report().await {
        Msg::EffectFailed { error } => {
            assert!(error.starts_with("Uploading the file failed: "), "{error}");
        }
        other => panic!("not a status: {other:?}"),
    }
    // It waits in the send queue for `message.resend` or its deletion.
    let queued = adapter.queued().await;
    assert!(
        matches!(
            queued.as_slice(),
            [LocalEcho {
                content: LocalEchoContent::Event {
                    send_error: Some(_),
                    ..
                },
                ..
            }]
        ),
        "{queued:?}"
    );
    adapter
        .core
        .until("the failed echo", |entries| {
            entries.iter().any(|entry| {
                matches!(entry, TimelineEntry::Item(TimelineItem {
                    content: MessageContent::File { name, .. },
                    send_state: Some(SendState::Failed { .. }),
                    ..
                }) if name == "notes.txt")
            })
        })
        .await;
}

#[tokio::test]
async fn a_refused_reaction_is_dropped_from_the_timeline_and_reported() {
    let mut adapter = Adapter::start().await;
    adapter
        .server
        .mock_room_send()
        .error_too_large()
        .mock_once()
        .mount()
        .await;

    adapter.effect(MatrixEffect::ToggleReaction {
        room: room(),
        item: sent(),
        key: "👍".to_owned(),
    });

    adapter
        .core
        .until("the reaction", |entries| reactions(entries) == ["👍"])
        .await;
    match adapter.core.report().await {
        Msg::EffectFailed { error } => assert!(error.starts_with("Reacting failed: "), "{error}"),
        other => panic!("not a status: {other:?}"),
    }
    adapter
        .core
        .until("the reaction's removal", |entries| {
            reactions(entries).is_empty()
        })
        .await;
    assert!(adapter.queued().await.is_empty());
}

/// The adapter, and secret storage with the key description of
/// matrix-sdk's mock examples, which `EXAMPLE_RECOVERY_KEY` opens.
async fn with_secret_storage() -> Adapter {
    let adapter = Adapter::start().await;
    let me = ruma::UserId::parse(ME).unwrap();
    let server = &adapter.server;
    server
        .mock_get_default_secret_storage_key()
        .ok(&me, "abc")
        .mount()
        .await;
    let properties = SecretStorageV1AesHmacSha2Properties::new(
        Some(Base64::parse("xv5b6/p3ExEw++wTyfSHEg==").unwrap()),
        Some(Base64::parse("ujBBbXahnTAMkmPUX2/0+VTfUh63pGyVRuBcDMgmJC8=").unwrap()),
    );
    let description = SecretStorageKeyEventContent::new(
        "abc".to_owned(),
        SecretStorageEncryptionAlgorithm::V1AesHmacSha2(properties),
    );
    server
        .mock_get_secret_storage_key()
        .ok(&me, &description)
        .mount()
        .await;
    adapter
}

/// The recovery key of the doc example of matrix-sdk's
/// `mock_get_secret_storage_key`.
const EXAMPLE_RECOVERY_KEY: &str = "EsTj 3yST y93F SLpB jJsz eAXc 2XzA ygD3 w69H fGaN TKBj jXEd";

const WRONG_KEY: &str = "Recovery failed: the recovery key is wrong";

/// The status line's text of the failure that answers `key`.
async fn recovery_failure(adapter: &mut Adapter, key: &str) -> String {
    adapter.effect(MatrixEffect::Recover(key.to_owned().into()));
    match adapter.core.report().await {
        Msg::EffectFailed { error } => error,
        other => panic!("not a failure: {other:?}"),
    }
}

#[tokio::test]
async fn a_wrong_recovery_key_is_reported_without_any_of_the_key() {
    let mut adapter = with_secret_storage().await;
    // The SDK's errors would show the first two bytes of the first key
    // (`[10, 50]`, not the prefix of a recovery key) and the `+` of the
    // second.
    for key in [
        "2222 2222 2222 2222 2222 2222 2222 2222 2222 2222 2222 2222",
        "made+up",
    ] {
        assert_eq!(recovery_failure(&mut adapter, key).await, WRONG_KEY);
    }
}

#[tokio::test]
async fn a_failure_after_the_right_recovery_key_is_not_a_wrong_key() {
    let mut adapter = with_secret_storage().await;
    // The key opens the secret storage; importing the secrets fails next,
    // since the mock homeserver has none.
    let error = recovery_failure(&mut adapter, EXAMPLE_RECOVERY_KEY).await;
    assert_ne!(error, WRONG_KEY);
}

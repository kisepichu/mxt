//! Threads (design, "Threads"): replies in threads are no entries of the
//! main timeline, and their root carries the thread's summary; a thread's
//! own timeline, sending and uploading into it, its receipts, and finding
//! the thread of an event. The client is built as mxt builds it, without
//! the SDK's thread support, which the design's risk "Thread features
//! without the SDK's thread support" relies on.

use std::num::NonZeroU32;

use matrix_sdk::{
    ruma::{
        api::client::receipt::create_receipt::v3::ReceiptType, events::receipt::ReceiptThread,
        mxc_uri,
    },
    test_utils::mocks::{RoomMessagesResponseTemplate, RoomRelationsResponseTemplate},
};
use mxt_core::{ids::Timestamp, timeline::ThreadSummary};
use serde_json::json;

use super::*;

const BOB: &str = "@bob:example.org";
const ROOT: &str = "$root:example.org";
/// Bob's first reply in the thread of `ROOT`.
const FIRST: &str = "$first:example.org";

fn remote(id: &str) -> ItemRef {
    ItemRef::Remote(EventId::new(id))
}

fn sdk_event(id: &str) -> ruma::OwnedEventId {
    ruma::EventId::parse(id).unwrap()
}

/// `ROOM`'s new events in a sync response, Bob's messages: each is its body
/// and event ID, its time, and, for a reply in the thread of `ROOT`, the
/// event its fallback points at.
async fn sync(adapter: &Adapter, messages: &[(&str, u64, Option<&str>)]) {
    let bob = ruma::UserId::parse(BOB).unwrap();
    let factory = EventFactory::new().sender(&bob);
    let room_id = ruma::RoomId::parse(ROOM).unwrap();
    let mut joined = JoinedRoomBuilder::new(&room_id);
    for &(id, ts, fallback) in messages {
        let mut message = factory.text_msg(id).event_id(&sdk_event(id)).server_ts(ts);
        if let Some(fallback) = fallback {
            message = message.in_thread(&sdk_event(ROOT), &sdk_event(fallback));
        }
        joined = joined.add_timeline_event(message);
    }
    adapter.server.sync_room(&adapter.client, joined).await;
}

/// The root's summary: `replies` replies, the latest at `latest`.
fn summary(replies: u32, latest: u64) -> Option<ThreadSummary> {
    Some(ThreadSummary {
        replies: NonZeroU32::new(replies).unwrap(),
        latest: Some(Timestamp::from_millis(latest)),
    })
}

fn root_summary(entries: &[TimelineEntry]) -> Option<ThreadSummary> {
    item(entries, &remote(ROOT))?.thread_summary
}

#[tokio::test]
async fn the_main_timeline_shows_a_thread_root_with_its_summary_and_not_its_replies() {
    let mut adapter = Adapter::without_rooms().await;
    let first = "$first:example.org";
    sync(&adapter, &[(ROOT, 1_000, None), (first, 2_000, Some(ROOT))]).await;
    adapter.effect(MatrixEffect::SubscribeTimeline(room()));
    adapter
        .core
        .until("the root's summary", |entries| {
            root_summary(entries) == summary(1, 2_000)
        })
        .await;
    assert!(item(&adapter.core.entries, &remote(first)).is_none());

    // A reply that arrives later changes the summary without a request, and
    // stays out of the timeline as a message after it comes in.
    let (second, after) = ("$second:example.org", "$after:example.org");
    sync(
        &adapter,
        &[(second, 3_000, Some(first)), (after, 4_000, None)],
    )
    .await;
    adapter
        .core
        .until("the new summary and the message after it", |entries| {
            root_summary(entries) == summary(2, 3_000) && item(entries, &remote(after)).is_some()
        })
        .await;
    assert!(item(&adapter.core.entries, &remote(second)).is_none());
    assert_eq!(
        adapter.core.timelines,
        HashSet::from([TimelineId::main(room())])
    );
}

#[tokio::test]
async fn back_pagination_answers_for_the_main_timeline_and_refuses_a_thread() {
    let mut adapter = Adapter::start().await;
    adapter
        .server
        .mock_room_messages()
        .ok(RoomMessagesResponseTemplate::default())
        .mount()
        .await;

    let thread = TimelineId {
        room: room(),
        thread: Some(EventId::new(ROOT)),
    };
    adapter.effect(MatrixEffect::PaginateBackwards(thread.clone()));
    assert_eq!(
        adapter.core.report().await,
        Msg::PaginationFailed {
            timeline: thread,
            error: Error::TimelineNotOpen.to_string(),
        }
    );
    adapter.effect(MatrixEffect::PaginateBackwards(TimelineId::main(room())));
    match adapter.core.report().await {
        Msg::PaginationDone { timeline, .. } => assert_eq!(timeline, TimelineId::main(room())),
        other => panic!("not the answer of the pagination: {other:?}"),
    }
}

/// The thread of `ROOT` in `ROOM`.
fn thread() -> TimelineId {
    TimelineId {
        room: room(),
        thread: Some(EventId::new(ROOT)),
    }
}

/// `ROOM` with Bob's root and his first reply in its thread synced, its
/// main timeline subscribed, and the thread subscribed with the reply in
/// its entries.
async fn with_thread() -> Adapter {
    let mut adapter = Adapter::without_rooms().await;
    sync(&adapter, &[(ROOT, 1_000, None), (FIRST, 2_000, Some(ROOT))]).await;
    adapter.effect(MatrixEffect::SubscribeTimeline(room()));
    adapter
        .core
        .until("the root", |entries| item(entries, &remote(ROOT)).is_some())
        .await;
    adapter.effect(MatrixEffect::SubscribeThread {
        room: room(),
        root: EventId::new(ROOT),
    });
    adapter
        .core
        .until_thread("the reply", |entries| {
            item(entries, &remote(FIRST)).is_some()
        })
        .await;
    adapter
}

/// A message from the composer with the plain body `text`, mentioning
/// `users`.
fn outgoing(text: &str, users: &[&str], kind: OutgoingKind) -> Outgoing {
    Outgoing {
        body: RenderedBody {
            plain: text.to_owned(),
            markdown: text.to_owned(),
            user_ids: users.iter().map(|user| UserId::new(*user)).collect(),
        },
        kind,
    }
}

/// Whether `entries` hold the user's message with `content` as sent.
fn sent_with(entries: &[TimelineEntry], content: impl Fn(&MessageContent) -> bool) -> bool {
    entries.iter().any(|entry| {
        matches!(entry, TimelineEntry::Item(item)
            if content(&item.content) && item.send_state == Some(SendState::Sent))
    })
}

/// Waits until the mock homeserver has received a request whose path
/// contains `part`.
async fn until_requested(adapter: &Adapter, part: &str) {
    let requested = timeout(WAIT, async {
        loop {
            let requests = adapter.server.server().received_requests().await;
            if requests
                .unwrap_or_default()
                .iter()
                .any(|request| request.url.path().contains(part))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(requested.is_ok(), "no request to {part}");
}

#[tokio::test]
async fn a_threads_timeline_holds_its_root_and_replies_once_its_start_loads() {
    let mut adapter = with_thread().await;
    adapter
        .server
        .mock_room_relations()
        .ok(RoomRelationsResponseTemplate::default())
        .mount()
        .await;
    adapter.effect(MatrixEffect::PaginateBackwards(thread()));
    match adapter.core.report().await {
        Msg::PaginationDone { timeline, .. } => assert_eq!(timeline, thread()),
        other => panic!("not the answer of the pagination: {other:?}"),
    }
    adapter
        .core
        .until_thread("the root", |entries| item(entries, &remote(ROOT)).is_some())
        .await;
    let items: Vec<&ItemRef> = adapter
        .core
        .thread
        .iter()
        .filter_map(|entry| match entry {
            TimelineEntry::Item(item) => Some(&item.id),
            _ => None,
        })
        .collect();
    assert_eq!(items, [&remote(ROOT), &remote(FIRST)]);
    assert!(adapter.core.timelines.contains(&thread()));
    assert!(item(&adapter.core.entries, &remote(FIRST)).is_none());
}

#[tokio::test]
async fn a_message_sent_into_a_thread_falls_back_to_its_latest_reply() {
    let mut adapter = with_thread().await;
    let carol = "@carol:example.org";
    adapter
        .server
        .mock_room_send()
        .body_matches_partial_json(json!({
            "body": "in the thread",
            "m.mentions": { "user_ids": [carol] },
            "m.relates_to": {
                "rel_type": "m.thread",
                "event_id": ROOT,
                "is_falling_back": true,
                "m.in_reply_to": { "event_id": FIRST },
            },
        }))
        .ok(event_id!("$new:example.org"))
        .expect(1)
        .mount()
        .await;
    let id = SendId::new(1);
    adapter.effect(MatrixEffect::Send {
        timeline: thread(),
        id,
        message: outgoing("in the thread", &[carol], OutgoingKind::New),
    });
    assert_eq!(adapter.core.report().await, Msg::SendQueued { id });
    adapter
        .core
        .until_thread("the sent message", |entries| {
            sent_with(entries, |content| content.plain_text() == "in the thread")
        })
        .await;
}

#[tokio::test]
async fn a_reply_in_a_thread_replies_within_it() {
    let mut adapter = with_thread().await;
    adapter
        .server
        .mock_room_send()
        .body_matches_partial_json(json!({
            "m.relates_to": {
                "rel_type": "m.thread",
                "event_id": ROOT,
                "m.in_reply_to": { "event_id": FIRST },
            },
        }))
        .ok(event_id!("$new:example.org"))
        .expect(1)
        .mount()
        .await;
    let id = SendId::new(1);
    let kind = OutgoingKind::Reply {
        to: EventId::new(FIRST),
    };
    adapter.effect(MatrixEffect::Send {
        timeline: thread(),
        id,
        message: outgoing("a reply", &[], kind),
    });
    assert_eq!(adapter.core.report().await, Msg::SendQueued { id });
    adapter
        .core
        .until_thread("the sent reply", |entries| {
            sent_with(entries, |content| content.plain_text() == "a reply")
        })
        .await;
    // ruma leaves `is_falling_back` out when it is false.
    let requests = adapter.server.server().received_requests().await;
    let relations: Vec<serde_json::Value> = requests
        .unwrap_or_default()
        .iter()
        .filter(|request| request.url.path().contains("/send/m.room.message/"))
        .map(|request| request.body_json::<serde_json::Value>().unwrap()["m.relates_to"].clone())
        .collect();
    assert_eq!(relations.len(), 1);
    assert!(
        matches!(
            relations[0].get("is_falling_back"),
            None | Some(serde_json::Value::Bool(false))
        ),
        "{relations:?}"
    );
}

#[tokio::test]
async fn sending_or_uploading_into_a_thread_that_is_not_open_fails() {
    let mut adapter = Adapter::start().await;
    let id = SendId::new(1);
    adapter.effect(MatrixEffect::Send {
        timeline: thread(),
        id,
        message: outgoing("lost", &[], OutgoingKind::New),
    });
    match adapter.core.report().await {
        Msg::SendFailed { id: failed, error } => {
            assert_eq!(failed, id);
            assert!(error.starts_with("Sending the message failed: "), "{error}");
        }
        other => panic!("not the answer of the send: {other:?}"),
    }
    adapter.effect(MatrixEffect::Upload {
        timeline: thread(),
        path: "/nonexistent/notes.txt".to_owned(),
    });
    match adapter.core.report().await {
        Msg::EffectFailed { error } => {
            assert!(error.starts_with("Uploading the file failed: "), "{error}");
        }
        other => panic!("not a status: {other:?}"),
    }
    assert!(adapter.queued().await.is_empty());
}

#[tokio::test]
async fn an_upload_into_a_thread_falls_back_to_its_latest_reply() {
    let mut adapter = with_thread().await;
    let server = &adapter.server;
    server
        .mock_authenticated_media_config()
        .ok_default()
        .mount()
        .await;
    server
        .mock_upload()
        .ok(mxc_uri!("mxc://example.org/notes"))
        .mount()
        .await;
    server
        .mock_room_send()
        .body_matches_partial_json(json!({
            "msgtype": "m.file",
            "m.relates_to": {
                "rel_type": "m.thread",
                "event_id": ROOT,
                "is_falling_back": true,
                "m.in_reply_to": { "event_id": FIRST },
            },
        }))
        .ok(event_id!("$file:example.org"))
        .expect(1)
        .mount()
        .await;
    let dir = TestDir::new("thread-upload");
    let path = dir.join("notes.txt");
    fs::write(&path, "notes").unwrap();

    adapter.effect(MatrixEffect::Upload {
        timeline: thread(),
        path: path.to_string_lossy().into_owned(),
    });
    adapter
        .core
        .until_thread("the sent file", |entries| {
            sent_with(entries, |content| {
                matches!(content, MessageContent::File { name, .. } if name == "notes.txt")
            })
        })
        .await;
}

#[tokio::test]
async fn an_effect_on_a_reply_in_a_thread_goes_through_the_threads_timeline() {
    let mut adapter = with_thread().await;
    adapter
        .server
        .mock_room_send()
        .body_matches_partial_json(json!({
            "m.relates_to": {
                "rel_type": "m.annotation",
                "event_id": FIRST,
                "key": "👍",
            },
        }))
        .ok(event_id!("$reaction:example.org"))
        .expect(1)
        .mount()
        .await;
    // The main timeline has no item of the reply.
    adapter.effect(MatrixEffect::ToggleReaction {
        room: room(),
        item: remote(FIRST),
        key: "👍".to_owned(),
    });
    adapter
        .core
        .until_thread("the reaction", |entries| {
            item(entries, &remote(FIRST)).is_some_and(|reply| {
                reply
                    .reactions
                    .iter()
                    .map(|group| group.key.as_str())
                    .eq(["👍"])
            })
        })
        .await;
    until_requested(&adapter, "/send/m.reaction/").await;
    adapter.server.server().verify().await;
}

#[tokio::test]
async fn a_threads_receipt_is_one_threaded_request() {
    let adapter = Adapter::start().await;
    let reply = event_id!("$reply:example.org");
    adapter
        .server
        .mock_send_receipt(ReceiptType::Read)
        .match_thread(ReceiptThread::Thread(sdk_event(ROOT)))
        .match_event_id(reply)
        .ok()
        .expect(1)
        .mount()
        .await;
    adapter.effect(MatrixEffect::SendThreadReceipt {
        room: room(),
        root: EventId::new(ROOT),
        event: EventId::new(reply.as_str()),
        unthreaded: false,
    });
    until_requested(&adapter, "/receipt/").await;
    adapter.server.server().verify().await;
}

#[tokio::test]
async fn a_threads_unthreaded_receipt_is_one_request_without_the_thread() {
    let adapter = Adapter::start().await;
    let reply = event_id!("$reply:example.org");
    adapter
        .server
        .mock_send_receipt(ReceiptType::Read)
        .match_thread(ReceiptThread::Thread(sdk_event(ROOT)))
        .ok()
        .expect(0)
        .mount()
        .await;
    adapter
        .server
        .mock_send_receipt(ReceiptType::Read)
        .match_event_id(reply)
        .body_json(json!({}))
        .ok()
        .expect(1)
        .mount()
        .await;
    adapter.effect(MatrixEffect::SendThreadReceipt {
        room: room(),
        root: EventId::new(ROOT),
        event: EventId::new(reply.as_str()),
        unthreaded: true,
    });
    until_requested(&adapter, "/receipt/").await;
    adapter.server.server().verify().await;
}

#[tokio::test]
async fn find_thread_answers_from_the_event_cache_without_a_request() {
    let mut adapter = Adapter::without_rooms().await;
    let plain = "$plain:example.org";
    sync(
        &adapter,
        &[
            (ROOT, 1_000, None),
            (FIRST, 2_000, Some(ROOT)),
            (plain, 3_000, None),
        ],
    )
    .await;
    // Once the main timeline has them, the event cache has them too.
    adapter.effect(MatrixEffect::SubscribeTimeline(room()));
    adapter
        .core
        .until("the messages", |entries| {
            item(entries, &remote(plain)).is_some()
        })
        .await;

    for (event, root) in [
        (FIRST, Some(ROOT)),
        (plain, None),
        (ROOT, None),
        ("$unknown:example.org", None),
    ] {
        adapter.effect(MatrixEffect::FindThread {
            room: room(),
            event: EventId::new(event),
        });
        assert_eq!(
            adapter.core.report().await,
            Msg::ThreadFound {
                room: room(),
                event: EventId::new(event),
                root: root.map(EventId::new),
            },
            "{event}"
        );
    }
    let requests = adapter.server.server().received_requests().await;
    let fetches: Vec<String> = requests
        .unwrap_or_default()
        .iter()
        .map(|request| request.url.path().to_owned())
        .filter(|path| path.contains("/event/") || path.contains("/context/"))
        .collect();
    assert_eq!(fetches, Vec::<String>::new());
}

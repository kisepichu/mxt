//! Threads in the main timeline (design, "Threads"): replies in threads are
//! no entries, and their root carries the thread's summary. The client is
//! built as mxt builds it, without the SDK's thread support, which the
//! design's risk "Thread features without the SDK's thread support" relies
//! on.

use std::num::NonZeroU32;

use matrix_sdk::test_utils::mocks::RoomMessagesResponseTemplate;
use mxt_core::{ids::Timestamp, timeline::ThreadSummary};

use super::*;

const BOB: &str = "@bob:example.org";
const ROOT: &str = "$root:example.org";

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

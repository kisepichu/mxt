//! `room <room_id> [--back N] [--wait SECS]`: questions (3) unread counts and
//! (7) keys from backup.

use std::{sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use matrix_sdk::{
    Room,
    deserialized_responses::TimelineEvent,
    ruma::{
        OwnedEventId, RoomId, UserId,
        events::receipt::{ReceiptThread, ReceiptType},
    },
};
use matrix_sdk_ui::timeline::{
    MsgLikeKind, RoomExt as _, Timeline, TimelineDetails, TimelineItem, TimelineItemContent,
};
use serde_json::Value;

use crate::{
    session::{self, DataDir},
    util,
};

/// Events to load at most while looking for the user's receipt.
const RECEIPT_SEARCH_LIMIT: usize = 500;

pub async fn run(dir: &DataDir, room_id: &RoomId, back: usize, wait: Duration) -> Result<()> {
    let session = session::restore(dir).await?;
    let client = &session.client;
    let sync = session::start_sync(client).await?;
    let room = session::wait_for_room(&sync, room_id, Duration::from_secs(60)).await?;
    let own_user_id = client.user_id().context("not logged in")?.to_owned();

    let sdk_unread_before = room.num_unread_messages();
    let timeline = room.timeline().await?;

    // Paginate until `back` event items are loaded or the start is reached.
    let mut at_start = false;
    while !at_start && event_items(&timeline).await.len() < back {
        at_start = timeline.paginate_backwards(20).await?;
    }
    let items = event_items(&timeline).await;
    for item in &items {
        print_item(item);
    }
    let utd_now = count_utd(&items);
    println!();
    println!(
        "{} event items loaded{}, {utd_now} undecryptable",
        items.len(),
        if at_start {
            " (start of the room reached)"
        } else {
            ""
        }
    );

    println!("waiting {wait:?} for keys from backup...");
    tokio::time::sleep(wait).await;
    session::check_sync(&sync)?;
    let items = event_items(&timeline).await;
    println!(
        "after {wait:?}: {} undecryptable of {} event items",
        count_utd(&items),
        items.len()
    );

    report_unread(&room, &timeline, &own_user_id, sdk_unread_before).await?;
    sync.stop().await;
    Ok(())
}

async fn event_items(timeline: &Timeline) -> Vec<Arc<TimelineItem>> {
    timeline
        .items()
        .await
        .into_iter()
        .filter(|item| item.as_event().is_some())
        .collect()
}

fn count_utd(items: &[Arc<TimelineItem>]) -> usize {
    items
        .iter()
        .filter_map(|item| item.as_event())
        .filter(|event| event.content().is_unable_to_decrypt())
        .count()
}

fn print_item(item: &TimelineItem) {
    let Some(event) = item.as_event() else {
        return;
    };
    let sender = match event.sender_profile() {
        TimelineDetails::Ready(profile) => match &profile.display_name {
            Some(name) => format!("{} ({name})", event.sender()),
            None => event.sender().to_string(),
        },
        _ => event.sender().to_string(),
    };
    let (kind, body) = describe(event.content());
    println!(
        "{} {sender} {kind}{} {}",
        util::format_ts(event.timestamp()),
        if event.content().is_unable_to_decrypt() {
            " UTD"
        } else {
            ""
        },
        util::preview(&body, 60)
    );
}

fn describe(content: &TimelineItemContent) -> (String, String) {
    match content {
        TimelineItemContent::MsgLike(msg_like) => match &msg_like.kind {
            MsgLikeKind::Message(message) => (
                message.msgtype().msgtype().to_owned(),
                message.body().to_owned(),
            ),
            MsgLikeKind::Sticker(_) => ("sticker".to_owned(), String::new()),
            MsgLikeKind::Poll(_) => ("poll".to_owned(), String::new()),
            MsgLikeKind::Redacted => ("redacted".to_owned(), String::new()),
            MsgLikeKind::UnableToDecrypt(_) => ("encrypted".to_owned(), String::new()),
            MsgLikeKind::Other(other) => (other.event_type().to_string(), String::new()),
            MsgLikeKind::LiveLocation(_) => ("live-location".to_owned(), String::new()),
        },
        TimelineItemContent::MembershipChange(_) => ("membership".to_owned(), String::new()),
        TimelineItemContent::ProfileChange(_) => ("profile".to_owned(), String::new()),
        TimelineItemContent::OtherState(state) => (
            format!("state {}", state.content().event_type()),
            String::new(),
        ),
        other => (
            other.event_type_str().unwrap_or_else(|| "other".to_owned()),
            String::new(),
        ),
    }
}

/// Compares the SDK's `num_unread_messages` with a count made from the loaded
/// events: messages from other users after the user's latest receipt.
async fn report_unread(
    room: &Room,
    timeline: &Timeline,
    own_user_id: &UserId,
    sdk_unread_before: u64,
) -> Result<()> {
    let mut receipt_ids: Vec<(String, OwnedEventId)> = Vec::new();
    for receipt_type in [ReceiptType::Read, ReceiptType::ReadPrivate] {
        for thread in [ReceiptThread::Unthreaded, ReceiptThread::Main] {
            if let Some((event_id, _)) = room
                .load_user_receipt(receipt_type.clone(), &thread, own_user_id)
                .await?
            {
                receipt_ids.push((format!("{receipt_type} ({thread:?})"), event_id));
            }
        }
    }
    if receipt_ids.is_empty() {
        println!("no own receipt is known for this room");
    }
    for (kind, event_id) in &receipt_ids {
        println!("own receipt {kind}: {event_id}");
    }

    let (event_cache, _drop_handles) = room.event_cache().await?;
    let mut events = event_cache.events().await?;
    let mut at_start = false;
    while latest_position(&events, &receipt_ids).is_none()
        && events.len() < RECEIPT_SEARCH_LIMIT
        && !at_start
    {
        at_start = timeline.paginate_backwards(50).await?;
        events = event_cache.events().await?;
    }

    println!(
        "SDK num_unread_messages: {sdk_unread_before} before loading, {} now (SDK active receipt: {})",
        room.num_unread_messages(),
        room.read_receipts()
            .latest_active
            .map_or_else(|| "none".to_owned(), |r| r.event_id.to_string()),
    );
    match latest_position(&events, &receipt_ids) {
        Some(position) => {
            let after_receipt = &events[position + 1..];
            let real = count_unread(after_receipt, own_user_id);
            // An own event after the receipt counts as an implicit receipt.
            let implicit = after_receipt
                .iter()
                .rposition(|e| e.sender().as_deref() == Some(own_user_id))
                .map_or(real, |own| {
                    count_unread(&after_receipt[own + 1..], own_user_id)
                });
            println!(
                "real count: {real} messages from others after the latest own receipt \
                 ({} events loaded, receipt at index {position}); {implicit} after the latest \
                 own receipt or own message",
                events.len()
            );
        }
        None => println!(
            "real count: receipt not found in {} loaded events{}; at least {} messages from others",
            events.len(),
            if at_start {
                " (start of the room reached)"
            } else {
                ""
            },
            count_unread(&events, own_user_id)
        ),
    }
    Ok(())
}

fn latest_position(events: &[TimelineEvent], receipts: &[(String, OwnedEventId)]) -> Option<usize> {
    events.iter().rposition(|event| {
        event
            .event_id()
            .is_some_and(|id| receipts.iter().any(|(_, receipt)| receipt == id))
    })
}

/// The SDK's rule for events that make a room unread (`marks_as_unread` in
/// matrix-sdk's `read_receipts.rs`): message-like types from others that are
/// not edits and not redacted.
fn count_unread(events: &[TimelineEvent], own_user_id: &UserId) -> usize {
    events
        .iter()
        .filter(|event| event.sender().as_deref() != Some(own_user_id))
        .filter(|event| {
            let Ok(value) = serde_json::from_str::<Value>(event.raw().json().get()) else {
                return false;
            };
            let interesting_type = matches!(
                value.get("type").and_then(Value::as_str),
                Some(
                    "m.room.message"
                        | "m.room.encrypted"
                        | "m.sticker"
                        | "m.poll.start"
                        | "org.matrix.msc3381.poll.start"
                        | "m.poll.end"
                        | "org.matrix.msc3381.poll.end"
                        | "m.message"
                )
            );
            let is_edit = value
                .pointer("/content/m.relates_to/rel_type")
                .and_then(Value::as_str)
                == Some("m.replace");
            let is_redacted = value.pointer("/unsigned/redacted_because").is_some();
            interesting_type && !is_edit && !is_redacted
        })
        .count()
}

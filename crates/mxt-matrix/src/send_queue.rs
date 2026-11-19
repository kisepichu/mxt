//! Requests that fail for good in a room's send queue: edits, reactions and
//! redactions are dropped, so they do not hold back the room's later
//! requests, and failed uploads are reported.

use std::collections::BTreeMap;

use matrix_sdk::{
    Client, QueueWedgeError,
    ruma::{
        OwnedRoomId, RoomId,
        events::{
            AnyMessageLikeEventContent,
            room::{encrypted::Relation, message::MessageType},
        },
    },
    send_queue::{LocalEcho, LocalEchoContent, RoomSendQueueUpdate, SendQueueUpdate},
    store::SerializableEventContent,
};
use mxt_core::msg::Msg;
use tokio::sync::{
    broadcast::{self, error::RecvError},
    mpsc::UnboundedSender,
};
use tracing::warn;

use crate::{adapter::report_brief, error::brief_sdk};

/// Edits, reactions and redactions are aggregations: the SDK applies them to
/// their target item at once, and they have no item of their own, so no
/// failed marker shows when sending one fails, and neither `message.resend`
/// nor deleting can reach it. One that fails with an unrecoverable error (a
/// refusal of the homeserver, for example) stays at the head of its room's
/// send queue, in the store, and holds back the room's later requests for
/// good. So it is aborted, which takes it off its target item, its room's
/// queue is enabled again, and the failure is reported. Recoverable failures
/// stay queued: they go out with the next request or when the sync connects.
/// Failed messages are left alone: their local echo shows the failure. A
/// failed upload is reported as well: it fails long after `:upload`, behind
/// the room's earlier requests, and its reason (a file above the
/// homeserver's limit, for example) is something the user can act on.
pub(crate) async fn handle_failures(
    client: Client,
    mut updates: broadcast::Receiver<SendQueueUpdate>,
    to_core: UnboundedSender<Msg>,
) {
    loop {
        match updates.recv().await {
            Ok(SendQueueUpdate {
                room_id,
                update:
                    RoomSendQueueUpdate::SendError {
                        transaction_id,
                        error,
                        is_recoverable: false,
                    },
            }) => {
                let echoes = local_echoes(&client).await.remove(&room_id);
                let failed = echoes
                    .iter()
                    .flatten()
                    .find(|echo| echo.transaction_id == transaction_id);
                if let Some(echo) = failed
                    && let Some(what) = drop_aggregation(&client, &room_id, echo)
                        .await
                        .or_else(|| upload(&echo.content))
                {
                    report_brief(&to_core, what, &brief_sdk(&error), &error);
                }
            }
            Ok(_) => {}
            // A missed update may have been such a failure; the store keeps
            // the error of a request that failed for good. A failed upload
            // keeps its local echo, which shows the failure.
            Err(RecvError::Lagged(_)) => {
                for (room_id, echoes) in local_echoes(&client).await {
                    for echo in &echoes {
                        if let Some(error) = wedge(&echo.content)
                            && let Some(what) = drop_aggregation(&client, &room_id, echo).await
                        {
                            report_brief(&to_core, what, &"it could not be sent", error);
                        }
                    }
                }
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// The requests in the rooms' send queues, by room; none when they cannot be
/// loaded.
async fn local_echoes(client: &Client) -> BTreeMap<OwnedRoomId, Vec<LocalEcho>> {
    client
        .send_queue()
        .local_echoes()
        .await
        .unwrap_or_else(|error| {
            warn!("Loading the send queues failed: {error}");
            BTreeMap::new()
        })
}

/// Aborts `echo` when it is an aggregation, and enables `room`'s queue again;
/// then what failed, for the report. `None` for a message, and for a request
/// that is gone already (the user removed the reaction meanwhile, for
/// example).
async fn drop_aggregation(
    client: &Client,
    room: &RoomId,
    echo: &LocalEcho,
) -> Option<&'static str> {
    let what = aggregation(&echo.content)?;
    let aborted = match &echo.content {
        LocalEchoContent::Event { send_handle, .. } => send_handle.abort().await,
        LocalEchoContent::React { send_handle, .. } => send_handle.abort().await,
        LocalEchoContent::Redaction { send_handle, .. } => send_handle.abort().await,
    };
    match aborted {
        Ok(true) => {
            if let Some(room) = client.get_room(room) {
                room.send_queue().set_enabled(true);
            }
        }
        Ok(false) => return None,
        // The failure is reported all the same.
        Err(error) => warn!("Dropping a failed request from the send queue failed: {error}"),
    }
    Some(what)
}

/// What failed, for the report, when `content` is an aggregation.
fn aggregation(content: &LocalEchoContent) -> Option<&'static str> {
    match content {
        LocalEchoContent::Event {
            serialized_event, ..
        } => event_aggregation(serialized_event),
        LocalEchoContent::React { .. } => Some("Reacting failed"),
        LocalEchoContent::Redaction { .. } => Some("Deleting the message failed"),
    }
}

/// Edits and reactions; messages, replies among them, are not aggregations.
fn event_aggregation(event: &SerializableEventContent) -> Option<&'static str> {
    match event.deserialize().ok()?.relation()? {
        Relation::Replacement(_) => Some("Editing the message failed"),
        Relation::Annotation(_) => Some("Reacting failed"),
        _ => None,
    }
}

/// What failed, for the report, when `content` is an upload.
fn upload(content: &LocalEchoContent) -> Option<&'static str> {
    match content {
        LocalEchoContent::Event {
            serialized_event, ..
        } => event_upload(serialized_event),
        LocalEchoContent::React { .. } | LocalEchoContent::Redaction { .. } => None,
    }
}

/// Messages with an attachment: `:upload` sends them.
fn event_upload(event: &SerializableEventContent) -> Option<&'static str> {
    let AnyMessageLikeEventContent::RoomMessage(message) = event.deserialize().ok()? else {
        return None;
    };
    matches!(
        message.msgtype,
        MessageType::Image(_)
            | MessageType::File(_)
            | MessageType::Video(_)
            | MessageType::Audio(_)
    )
    .then_some("Uploading the file failed")
}

/// The error of a request that failed for good. A reaction to a local echo
/// waits for its target, and is then sent as an event of its own.
fn wedge(content: &LocalEchoContent) -> Option<&QueueWedgeError> {
    match content {
        LocalEchoContent::Event { send_error, .. }
        | LocalEchoContent::Redaction { send_error, .. } => send_error.as_ref(),
        LocalEchoContent::React { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::{
        events::{
            reaction::ReactionEventContent,
            relation::{Annotation, Reply},
            room::message::{
                self, FileMessageEventContent, ReplacementMetadata, RoomMessageEventContent,
            },
        },
        owned_event_id, owned_mxc_uri,
    };

    use super::*;

    fn aggregation_of(content: impl Into<AnyMessageLikeEventContent>) -> Option<&'static str> {
        event_aggregation(&SerializableEventContent::new(&content.into()).unwrap())
    }

    #[test]
    fn edits_and_reactions_are_aggregations_and_messages_are_not() {
        let target = owned_event_id!("$target:example.org");
        let edit = RoomMessageEventContent::text_plain("fixed")
            .make_replacement(ReplacementMetadata::new(target.clone(), None));
        assert_eq!(aggregation_of(edit), Some("Editing the message failed"));
        let reaction = ReactionEventContent::new(Annotation::new(target.clone(), "👍".into()));
        assert_eq!(aggregation_of(reaction), Some("Reacting failed"));
        // A failed message keeps its local echo, for `message.resend`.
        assert_eq!(
            aggregation_of(RoomMessageEventContent::text_plain("hi")),
            None
        );
        let mut reply = RoomMessageEventContent::text_plain("hi");
        reply.relates_to = Some(message::Relation::Reply(Reply::with_event_id(target)));
        assert_eq!(aggregation_of(reply), None);
    }

    #[test]
    fn failed_attachments_are_uploads_and_text_messages_are_not() {
        let event = |content: RoomMessageEventContent| {
            SerializableEventContent::new(&content.into()).unwrap()
        };
        let file = FileMessageEventContent::plain(
            "notes.pdf".to_owned(),
            owned_mxc_uri!("mxc://send-queue.localhost/notes"),
        );
        let file = RoomMessageEventContent::new(MessageType::File(file));
        assert_eq!(
            event_upload(&event(file)),
            Some("Uploading the file failed")
        );
        let text = RoomMessageEventContent::text_plain("hi");
        assert_eq!(event_upload(&event(text)), None);
    }
}

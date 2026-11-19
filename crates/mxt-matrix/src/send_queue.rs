//! Requests that fail for good in a room's send queue: edits, reactions and
//! redactions are dropped, so they do not hold back the room's later
//! requests, and failed uploads are reported. The edits of sent messages
//! that the composer sends are answered when they are sent or fail.

use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    sync::Arc,
};

use matrix_sdk::{
    Client, QueueWedgeError,
    ruma::{
        OwnedRoomId, OwnedTransactionId, RoomId, TransactionId,
        events::{
            AnyMessageLikeEventContent,
            room::{encrypted::Relation, message::MessageType},
        },
    },
    send_queue::{LocalEcho, LocalEchoContent, RoomSendQueueUpdate, SendQueueUpdate},
    store::SerializableEventContent,
};
use matrix_sdk_ui::{Timeline, timeline};
use mxt_core::{ids::SendId, msg::Msg};
use tokio::sync::{
    Mutex,
    broadcast::{
        self,
        error::{RecvError, TryRecvError},
    },
    mpsc::UnboundedSender,
};
use tracing::{debug, warn};

use crate::{
    adapter::{report_brief, report_send_brief},
    error::brief_sdk,
};

/// The report of an edit that the homeserver refused.
const EDIT_FAILED: &str = "Editing the message failed";

/// The edits of sent messages that the composer sent in this session, by
/// the transaction ID of their request, with the core's `SendId`. Such an
/// edit has no local echo of its own that would keep its text: it is a
/// request in the send queue, dropped when it fails for good. So its send is
/// answered only when it leaves the send queue (design, "Error handling"):
/// `Msg::SendQueued` when it is sent, `Msg::SendFailed` when it fails for
/// good, which puts its draft back. `handle_failures` answers.
#[derive(Clone, Default)]
pub(crate) struct Edits(Arc<Mutex<HashMap<OwnedTransactionId, SendId>>>);

impl Edits {
    /// Queues `edit` through `timeline` as the core's send `id`; `true` when
    /// it is recorded, `false` when its request was not found, so the send
    /// is answered at once. The SDK's `SendHandle` does not expose its
    /// transaction ID, so the request is found by its content among the send
    /// queue's updates: `Timeline::send` reports it as a `NewLocalEvent`
    /// before it returns. The lock is held from queueing the edit until it
    /// is recorded, so `handle_failures` cannot look up its outcome in
    /// between.
    pub(crate) async fn send(
        &self,
        timeline: &Timeline,
        edit: AnyMessageLikeEventContent,
        id: SendId,
    ) -> Result<bool, timeline::Error> {
        // As the send queue stores it: the SDK serializes it the same way.
        let stored = SerializableEventContent::new(&edit).ok();
        let room = timeline.room();
        let mut edits = self.0.lock().await;
        let mut updates = room.client().send_queue().subscribe();
        timeline.send(edit).await?;
        loop {
            match updates.try_recv() {
                Ok(SendQueueUpdate {
                    room_id,
                    update: RoomSendQueueUpdate::NewLocalEvent(echo),
                }) if room_id == room.room_id() && is_event(&echo.content, stored.as_ref()) => {
                    edits.insert(echo.transaction_id, id);
                    return Ok(true);
                }
                Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => {
                    debug!("The request of an edit is not among the send queue's updates");
                    return Ok(false);
                }
            }
        }
    }

    /// The `SendId` of the recorded edit `txn`, which is forgotten.
    async fn take(&self, txn: &TransactionId) -> Option<SendId> {
        self.0.lock().await.remove(txn)
    }
}

/// Whether `echo` is an event stored as `stored`.
fn is_event(echo: &LocalEchoContent, stored: Option<&SerializableEventContent>) -> bool {
    let (
        LocalEchoContent::Event {
            serialized_event, ..
        },
        Some(stored),
    ) = (echo, stored)
    else {
        return false;
    };
    serialized_event.raw().0.json().get() == stored.raw().0.json().get()
}

/// Edits, reactions and redactions are aggregations: the SDK applies them to
/// their target item at once, and they have no item of their own, so no
/// failed marker shows when sending one fails, and neither `message.resend`
/// nor deleting can reach it. One that fails with an unrecoverable error (a
/// refusal of the homeserver, for example) stays at the head of its room's
/// send queue, in the store, and holds back the room's later requests for
/// good. So it is aborted, which takes a reaction off its target item
/// (matrix-sdk-ui 0.19 leaves an aborted edit's text on its target when no
/// other edit applies), its room's queue is enabled again, and the failure
/// is reported: for an edit in `edits`, as the answer to its send.
/// Recoverable failures stay queued: they go out with the next request or
/// when the sync connects. Failed messages are left alone: their local echo
/// shows the failure. A failed upload is reported as well: it fails long
/// after `:upload`, behind the room's earlier requests, and its reason (a
/// file above the homeserver's limit, for example) is something the user can
/// act on. An edit in `edits` that is sent is answered too.
pub(crate) async fn handle_failures(
    client: Client,
    edits: Edits,
    mut updates: broadcast::Receiver<SendQueueUpdate>,
    to_core: UnboundedSender<Msg>,
) {
    loop {
        match updates.recv().await {
            Ok(SendQueueUpdate {
                update: RoomSendQueueUpdate::SentEvent { transaction_id, .. },
                ..
            }) => {
                if let Some(id) = edits.take(&transaction_id).await {
                    let _ = to_core.send(Msg::SendQueued { id });
                }
            }
            Ok(SendQueueUpdate {
                room_id,
                update:
                    RoomSendQueueUpdate::SendError {
                        transaction_id,
                        error,
                        is_recoverable: false,
                    },
            }) => {
                let edit = edits.take(&transaction_id).await;
                let echoes = local_echoes(&client)
                    .await
                    .and_then(|mut queues| queues.remove(&room_id));
                let failed = echoes
                    .iter()
                    .flatten()
                    .find(|echo| echo.transaction_id == transaction_id);
                let what = match failed {
                    Some(echo) => drop_aggregation(&client, &room_id, echo)
                        .await
                        .or_else(|| upload(&echo.content)),
                    None => None,
                };
                report_failure(&to_core, edit, what, &brief_sdk(&error), &error);
            }
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => after_lag(&client, &edits, &to_core).await,
            Err(RecvError::Closed) => return,
        }
    }
}

/// After missed updates: a request that failed for good meanwhile is handled
/// as above, since the store keeps its error (a failed upload keeps its
/// local echo, which shows the failure), and an edit in `edits` that is no
/// longer queued was sent. `edits` stays locked throughout, so no edit is
/// recorded between loading the queues and answering the ones that left
/// them.
async fn after_lag(client: &Client, edits: &Edits, to_core: &UnboundedSender<Msg>) {
    let mut recorded = edits.0.lock().await;
    let Some(queues) = local_echoes(client).await else {
        return;
    };
    for (room_id, echoes) in &queues {
        for echo in echoes {
            if let Some(error) = wedge(&echo.content) {
                let what = drop_aggregation(client, room_id, echo).await;
                let edit = recorded.remove(&echo.transaction_id);
                report_failure(to_core, edit, what, &"it could not be sent", error);
            }
        }
    }
    recorded.retain(|txn, id| {
        let queued = queues
            .values()
            .flatten()
            .any(|echo| echo.transaction_id == *txn);
        if !queued {
            let _ = to_core.send(Msg::SendQueued { id: *id });
        }
        queued
    });
}

/// Reports a request that failed for good: an edit in `Edits` as
/// `Msg::SendFailed`, so the core can put its draft back; otherwise `what`,
/// if anything.
fn report_failure(
    to_core: &UnboundedSender<Msg>,
    edit: Option<SendId>,
    what: Option<&str>,
    brief: &dyn Display,
    full: &dyn Display,
) {
    match (edit, what) {
        (Some(id), _) => report_send_brief(to_core, id, EDIT_FAILED, brief, full),
        (None, Some(what)) => report_brief(to_core, what, brief, full),
        (None, None) => {}
    }
}

/// The requests in the rooms' send queues, by room; `None` when they cannot
/// be loaded.
async fn local_echoes(client: &Client) -> Option<BTreeMap<OwnedRoomId, Vec<LocalEcho>>> {
    client
        .send_queue()
        .local_echoes()
        .await
        .inspect_err(|error| warn!("Loading the send queues failed: {error}"))
        .ok()
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
        Relation::Replacement(_) => Some(EDIT_FAILED),
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

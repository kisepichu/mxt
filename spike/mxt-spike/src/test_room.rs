//! `create-test-room` and `send`: the only writing commands, guarded by the
//! test room recorded in `<data dir>/test-room`.

use std::{fs, time::Duration};

use anyhow::{Context as _, Result, bail};
use matrix_sdk::{
    ruma::{
        EventEncryptionAlgorithm, OwnedEventId, OwnedRoomId, OwnedUserId, RoomId,
        api::client::room::{Visibility, create_room::v3 as create_room},
        events::{
            InitialStateEvent, Mentions,
            room::{encryption::RoomEncryptionEventContent, message::RoomMessageEventContent},
        },
    },
    send_queue::RoomSendQueueUpdate,
};
use matrix_sdk_ui::timeline::RoomExt as _;
use tokio::sync::broadcast;

use crate::{
    session::{self, DataDir},
    util,
};

/// The test room recorded by `create-test-room`, if any.
fn recorded_test_room(dir: &DataDir) -> Result<Option<OwnedRoomId>> {
    let path = dir.test_room_file();
    match fs::read_to_string(&path) {
        Ok(contents) => {
            Ok(Some(contents.trim().to_owned().try_into().with_context(
                || format!("{} does not hold a room ID", path.display()),
            )?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

pub async fn create(dir: &DataDir) -> Result<()> {
    if let Some(room_id) = recorded_test_room(dir)? {
        println!("{room_id}");
        return Ok(());
    }
    let session = session::restore(dir).await?;
    let mut request = create_room::Request::new();
    request.name = Some("mxt-test".to_owned());
    request.preset = Some(create_room::RoomPreset::PrivateChat);
    request.visibility = Visibility::Private;
    request.initial_state = vec![
        InitialStateEvent::with_empty_state_key(RoomEncryptionEventContent::new(
            EventEncryptionAlgorithm::MegolmV1AesSha2,
        ))
        .to_raw_any(),
    ];
    let room = session.client.create_room(request).await?;
    session::write_new_private(
        &dir.test_room_file(),
        format!("{}\n", room.room_id()).as_bytes(),
    )?;
    println!("{}", room.room_id());
    Ok(())
}

pub struct SendArgs<'a> {
    pub room_id: &'a RoomId,
    pub text: &'a str,
    pub mention: Option<&'a (OwnedUserId, String)>,
    pub allow_bridged: bool,
}

pub async fn send(dir: &DataDir, args: SendArgs<'_>) -> Result<()> {
    // The guard runs before the session is restored, so a refused send never
    // touches the network.
    if !args.allow_bridged {
        match recorded_test_room(dir)? {
            Some(test_room) if test_room == args.room_id => {}
            Some(test_room) => bail!(
                "refusing to send to {}: only the test room {test_room} is allowed without \
                 --allow-bridged",
                args.room_id
            ),
            None => bail!(
                "refusing to send to {}: no test room is recorded in {} (run create-test-room), \
                 and --allow-bridged was not given",
                args.room_id,
                dir.test_room_file().display()
            ),
        }
    }
    let content = match args.mention {
        Some((user_id, mention_text)) => {
            let html = util::mention_html(args.text, mention_text, user_id).with_context(|| {
                format!("the mention text `{mention_text}` does not occur in the message")
            })?;
            RoomMessageEventContent::text_html(args.text, html)
                .add_mentions(Mentions::with_user_ids([user_id.clone()]))
        }
        None => RoomMessageEventContent::text_plain(args.text),
    };

    let session = session::restore(dir).await?;
    let sync = session::start_sync(&session.client).await?;
    let room = session::wait_for_room(&sync, args.room_id, Duration::from_secs(60)).await?;
    let timeline = room.timeline().await?;

    let (_, updates) = room.send_queue().subscribe().await?;
    timeline.send(content.into()).await?;
    let result = tokio::time::timeout(Duration::from_secs(60), wait_until_sent(updates)).await;
    sync.stop().await;
    let event_id = result.context("the local echo was not sent within 60s")??;
    println!("{event_id}");
    Ok(())
}

/// Waits for the first local echo queued after subscribing (ours) to be sent.
async fn wait_until_sent(
    mut updates: broadcast::Receiver<RoomSendQueueUpdate>,
) -> Result<OwnedEventId> {
    let mut ours = None;
    loop {
        match updates.recv().await? {
            RoomSendQueueUpdate::NewLocalEvent(echo) if ours.is_none() => {
                ours = Some(echo.transaction_id);
            }
            RoomSendQueueUpdate::SentEvent {
                transaction_id,
                event_id,
            } if ours.as_ref() == Some(&transaction_id) => return Ok(event_id),
            RoomSendQueueUpdate::SendError {
                transaction_id,
                error,
                is_recoverable,
            } if ours.as_ref() == Some(&transaction_id) => {
                bail!("sending failed (recoverable: {is_recoverable}): {error}")
            }
            _ => {}
        }
    }
}

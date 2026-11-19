//! The classic `/sync` loop (design, "Matrix adapter").

use std::{sync::Arc, time::Duration};

use matrix_sdk::{
    Client,
    config::{SyncSettings, SyncToken},
    ruma::{
        OwnedRoomId,
        api::client::{filter::FilterDefinition, sync::sync_events::v3::Filter},
        presence::PresenceState,
        uint,
    },
    store::StateStoreDataKey,
};
use mxt_core::msg::SyncState;
use tokio::{
    sync::{broadcast, watch},
    task::{JoinError, JoinSet},
};
use tracing::{debug, warn};

use crate::{
    Error,
    adapter::first_panic,
    backoff::Backoff,
    error::{is_unreachable, without_url},
};

/// How long the homeserver may hold a sync request open without news. The
/// SDK adds it to the client's 30 s request timeout, so a request has 60 s.
const LONG_POLL: Duration = Duration::from_secs(30);
/// The long-poll timeout of a request without a stored sync token. Such an
/// initial sync returns the full state, so the homeserver answers as soon as
/// it has computed it; the value only gives the request its HTTP time budget:
/// 150 s for a large account.
const INITIAL_POLL: Duration = Duration::from_secs(120);
/// Sync responses the room list may fall behind by before it re-reads every
/// room.
const RESPONSES_CAPACITY: usize = 16;

/// The joined and left rooms of one sync response.
pub(crate) type SyncedRooms = Arc<[OwnedRoomId]>;

/// The classic sync loop. Started once per process; the first login starts
/// it early so the device keys reach the server before the recovery prompt.
pub struct SyncLoop {
    task: JoinSet<()>,
    state: watch::Receiver<SyncState>,
    rooms: broadcast::Sender<SyncedRooms>,
}

impl SyncLoop {
    /// Subscribes the event cache (the SDK computes unread counts and latest
    /// events from it), then runs `Client::sync_once` in a loop with the
    /// stored sync token, a long-poll timeout (see `Requests::timeout`),
    /// presence offline, and a filter with lazy-loaded members, a timeline
    /// limit of 10, and neither presence nor typing notifications.
    pub fn start(client: &Client) -> Result<SyncLoop, Error> {
        client
            .event_cache()
            .subscribe()
            .map_err(matrix_sdk::Error::from)?;
        let (report, state) = watch::channel(SyncState::Starting);
        let (rooms, _) = broadcast::channel(RESPONSES_CAPACITY);
        let mut task = JoinSet::new();
        task.spawn(run(client.clone(), report, rooms.clone()));
        Ok(SyncLoop { task, state, rooms })
    }

    /// The sync state; it changes only when the state does.
    pub(crate) fn state(&self) -> watch::Receiver<SyncState> {
        self.state.clone()
    }

    /// The rooms of each sync response from now on.
    pub(crate) fn rooms(&self) -> broadcast::Receiver<SyncedRooms> {
        self.rooms.subscribe()
    }

    /// See `Matrix::panicked`.
    pub(crate) async fn panicked(&mut self) -> JoinError {
        first_panic(&mut self.task).await
    }

    /// Stops the loop and waits until it has stopped.
    ///
    /// A response that is being processed is cut short: the SDK handles its
    /// to-device events before it stores the state and the sync token, so
    /// the next start receives that batch again (most likely during the large
    /// first sync). That is accepted: the crypto store has saved either the
    /// batch's room keys or nothing of it, so the repeat costs warnings about
    /// Olm messages that do not decrypt twice, not keys. `sync_once` does not
    /// separate the request from the processing, so waiting for the
    /// processing would mean waiting for the request too: quitting could take
    /// a whole long poll.
    pub async fn stop(mut self) {
        self.task.shutdown().await;
    }
}

async fn run(
    client: Client,
    state: watch::Sender<SyncState>,
    rooms: broadcast::Sender<SyncedRooms>,
) {
    let settings = settings();
    let mut requests = Requests::new(has_sync_token(&client).await);
    loop {
        match client
            .sync_once(settings.clone().timeout(requests.timeout()))
            .await
        {
            Ok(response) => {
                requests.succeeded();
                set_state(&state, SyncState::Synced);
                // Before `Matrix::start` nobody listens; the room list reads
                // every room when it starts.
                if rooms.receiver_count() == 0 {
                    continue;
                }
                let updated: SyncedRooms = response
                    .rooms
                    .joined
                    .keys()
                    .chain(response.rooms.left.keys())
                    .cloned()
                    .collect();
                if !updated.is_empty() {
                    let _ = rooms.send(updated);
                }
            }
            Err(error) => {
                let error = Error::from(without_url(error));
                match requests.failed(failure(&error)) {
                    AfterFailure::Stop => {
                        warn!("the homeserver no longer accepts this session; the sync stops");
                        return;
                    }
                    AfterFailure::Retry { state: next, delay } => {
                        set_state(&state, next);
                        warn!(
                            "the sync failed ({}); retrying in {} s",
                            error.brief(),
                            delay.as_secs()
                        );
                        debug!("the sync failed: {error}");
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
    }
}

/// `SyncToken::ReusePrevious`: `sync_once` stores each response's token, so
/// every request uses the same settings, apart from the timeout.
fn settings() -> SyncSettings {
    let mut filter = FilterDefinition::with_lazy_loading();
    filter.room.timeline.limit = Some(uint!(10));
    // mxt shows neither presence nor typing. In bridged rooms the puppets
    // send both, and each would end the long poll and re-read the room. Read
    // receipts stay: the unread counts need them.
    filter.presence.not_types = vec!["*".to_owned()];
    filter.room.ephemeral.not_types = vec!["m.typing".to_owned()];
    SyncSettings::new()
        .token(SyncToken::ReusePrevious)
        .set_presence(PresenceState::Offline)
        .filter(Filter::FilterDefinition(filter))
}

/// Whether an earlier run stored a sync token. A store error counts as none:
/// the first request then only gets the larger time budget.
async fn has_sync_token(client: &Client) -> bool {
    matches!(
        client
            .state_store()
            .get_kv_data(StateStoreDataKey::SyncToken)
            .await,
        Ok(Some(_))
    )
}

fn set_state(state: &watch::Sender<SyncState>, next: SyncState) {
    state.send_if_modified(|current| std::mem::replace(current, next) != next);
}

/// Why a sync request failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// The request did not reach the homeserver, or its answer did not
    /// arrive.
    Unreachable,
    /// The homeserver no longer accepts the access token.
    UnknownToken,
    /// An error response, or a response the SDK could not read or store.
    Other,
}

fn failure(error: &Error) -> Failure {
    match error {
        Error::UnknownToken => Failure::UnknownToken,
        Error::Sdk(error) if is_unreachable(error) => Failure::Unreachable,
        _ => Failure::Other,
    }
}

/// What the loop does after a failed request.
#[derive(Debug, PartialEq, Eq)]
enum AfterFailure {
    /// Report `state`, then send the next request after `delay`.
    Retry { state: SyncState, delay: Duration },
    /// The session is gone; the session watch ends mxt.
    Stop,
}

/// The sync state after failures, the retry delay (1 s, doubling, at most
/// 60 s, reset by a response) and the long-poll timeout of each request.
struct Requests {
    backoff: Backoff,
    /// Whether a sync token is stored, so requests continue from it.
    token: bool,
}

impl Requests {
    fn new(token: bool) -> Requests {
        Requests {
            backoff: Backoff::new(Duration::from_secs(1), Duration::from_secs(60)),
            token,
        }
    }

    /// The long poll, also right after a failure: the homeserver answers as
    /// soon as there is news, and a catch-up after an outage may need the
    /// whole time budget. Until a token is stored, the initial sync's.
    fn timeout(&self) -> Duration {
        if self.token { LONG_POLL } else { INITIAL_POLL }
    }

    fn succeeded(&mut self) {
        self.backoff.reset();
        self.token = true;
    }

    fn failed(&mut self, failure: Failure) -> AfterFailure {
        let state = match failure {
            Failure::UnknownToken => return AfterFailure::Stop,
            Failure::Unreachable => SyncState::Offline,
            Failure::Other => SyncState::Retrying,
        };
        AfterFailure::Retry {
            state,
            delay: self.backoff.next_delay(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry(state: SyncState, seconds: u64) -> AfterFailure {
        AfterFailure::Retry {
            state,
            delay: Duration::from_secs(seconds),
        }
    }

    #[test]
    fn failures_map_to_a_sync_state_and_back_off_until_a_response() {
        let mut requests = Requests::new(true);
        assert_eq!(
            requests.failed(Failure::Unreachable),
            retry(SyncState::Offline, 1)
        );
        assert_eq!(
            requests.failed(Failure::Other),
            retry(SyncState::Retrying, 2)
        );
        assert_eq!(
            requests.failed(Failure::Unreachable),
            retry(SyncState::Offline, 4)
        );
        requests.succeeded();
        assert_eq!(
            requests.failed(Failure::Other),
            retry(SyncState::Retrying, 1)
        );
        assert_eq!(requests.failed(Failure::UnknownToken), AfterFailure::Stop);
    }

    #[test]
    fn requests_keep_the_initial_sync_budget_until_a_token_is_stored() {
        let mut requests = Requests::new(false);
        assert_eq!(requests.timeout(), INITIAL_POLL);
        requests.failed(Failure::Unreachable);
        assert_eq!(requests.timeout(), INITIAL_POLL);
        requests.succeeded();
        assert_eq!(requests.timeout(), LONG_POLL);
        // A catch-up after a failure long-polls as well.
        requests.failed(Failure::Unreachable);
        assert_eq!(requests.timeout(), LONG_POLL);
        assert_eq!(Requests::new(true).timeout(), LONG_POLL);
    }
}

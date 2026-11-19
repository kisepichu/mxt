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

/// How long the homeserver may hold a sync request open without news.
const LONG_POLL: Duration = Duration::from_secs(30);
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
    /// stored sync token, a 30 s long-poll timeout (see `Requests::timeout`),
    /// presence offline, and a filter with lazy-loaded members and a
    /// timeline limit of 10.
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
    let mut requests = Requests::new();
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
    SyncSettings::new()
        .token(SyncToken::ReusePrevious)
        .set_presence(PresenceState::Offline)
        .filter(Filter::FilterDefinition(filter))
}

fn set_state(state: &watch::Sender<SyncState>, next: SyncState) {
    state.send_if_modified(|current| std::mem::replace(current, next) != next);
}

/// Why a sync request failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// The request did not reach the homeserver, or its answer did not
    /// arrive in time.
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
    /// Whether a request has succeeded since the start, so a sync token is
    /// stored.
    responded: bool,
    /// Whether the last request failed.
    failing: bool,
}

impl Requests {
    fn new() -> Requests {
        Requests {
            backoff: Backoff::new(Duration::from_secs(1), Duration::from_secs(60)),
            responded: false,
            failing: false,
        }
    }

    /// The long poll, except right after a failure once a token is stored:
    /// then the homeserver answers at once, so the state shows without delay
    /// that it is reachable again. The SDK adds the timeout to the request's
    /// own 30 s, and an initial sync (without a token) needs that time on a
    /// large account; the homeserver answers it at once anyway.
    fn timeout(&self) -> Duration {
        if self.responded && self.failing {
            Duration::ZERO
        } else {
            LONG_POLL
        }
    }

    fn succeeded(&mut self) {
        self.backoff.reset();
        self.responded = true;
        self.failing = false;
    }

    fn failed(&mut self, failure: Failure) -> AfterFailure {
        self.failing = true;
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
        let mut requests = Requests::new();
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
    fn only_a_retry_after_a_response_skips_the_long_poll() {
        let mut requests = Requests::new();
        assert_eq!(requests.timeout(), LONG_POLL);
        // No token may be stored yet: an initial sync keeps the time.
        requests.failed(Failure::Unreachable);
        assert_eq!(requests.timeout(), LONG_POLL);
        requests.succeeded();
        assert_eq!(requests.timeout(), LONG_POLL);
        requests.failed(Failure::Unreachable);
        assert_eq!(requests.timeout(), Duration::ZERO);
        requests.failed(Failure::Other);
        assert_eq!(requests.timeout(), Duration::ZERO);
        requests.succeeded();
        assert_eq!(requests.timeout(), LONG_POLL);
    }
}

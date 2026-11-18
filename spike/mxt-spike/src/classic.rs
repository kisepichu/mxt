//! `classic-probe`: question (1), whether classic `/sync` is a usable fallback.
//!
//! One initial `/sync` request sent with `Client::send`, so nothing reaches the
//! SDK's store; without a `since` token the server acknowledges nothing.

use std::time::{Duration, Instant};

use anyhow::Result;
use matrix_sdk::{
    config::RequestConfig,
    ruma::{
        UInt,
        api::client::{
            filter::{FilterDefinition, LazyLoadOptions},
            sync::sync_events::v3,
        },
        presence::PresenceState,
    },
};

use crate::session::{self, DataDir};

pub async fn run(dir: &DataDir) -> Result<()> {
    let session = session::restore(dir).await?;

    let mut filter = FilterDefinition::default();
    filter.room.timeline.limit = Some(UInt::from(1u32));
    let lazy = LazyLoadOptions::Enabled {
        include_redundant_members: false,
    };
    filter.room.timeline.lazy_load_options = lazy;
    filter.room.state.lazy_load_options = lazy;

    let mut request = v3::Request::new();
    request.filter = Some(v3::Filter::FilterDefinition(filter));
    request.timeout = Some(Duration::ZERO);
    request.set_presence = PresenceState::Offline;

    let start = Instant::now();
    let result = session
        .client
        .send(request)
        .with_request_config(
            RequestConfig::new()
                .timeout(Duration::from_secs(600))
                .disable_retry(),
        )
        .await;
    let elapsed = start.elapsed();
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            println!(
                "classic /sync failed after {:.1}s: {error}",
                elapsed.as_secs_f64()
            );
            if let Some(status) = error.as_client_api_error().map(|e| e.status_code) {
                println!("HTTP status: {status}");
            }
            return Ok(());
        }
    };

    // The body itself is not available after deserialization; re-serializing
    // the parsed parts gives its decoded JSON size (events stay raw JSON).
    let size = serde_json::to_vec(&response.rooms)?.len()
        + serde_json::to_vec(&response.presence)?.len()
        + serde_json::to_vec(&response.account_data)?.len()
        + serde_json::to_vec(&response.to_device)?.len()
        + serde_json::to_vec(&response.device_lists)?.len();
    let timeline_events: usize = response
        .rooms
        .join
        .values()
        .map(|room| room.timeline.events.len())
        .sum();
    let state_events: usize = response
        .rooms
        .join
        .values()
        .map(|room| match &room.state {
            v3::State::Before(state) | v3::State::After(state) => state.events.len(),
            _ => 0,
        })
        .sum();
    println!(
        "classic /sync: status 200 after {:.1}s",
        elapsed.as_secs_f64()
    );
    println!(
        "response size: about {:.1} MiB of JSON",
        size as f64 / (1024.0 * 1024.0)
    );
    println!(
        "joined rooms: {}, invited: {}, left: {}",
        response.rooms.join.len(),
        response.rooms.invite.len(),
        response.rooms.leave.len()
    );
    println!("timeline events: {timeline_events}, state events: {state_events}");
    Ok(())
}

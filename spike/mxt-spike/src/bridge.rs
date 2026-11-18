//! Bridge state from one-off MSC4186 requests sent with `Client::send`.
//!
//! The responses are parsed here and never reach the SDK's sync processing,
//! store or event cache (design, "Matrix adapter").

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use matrix_sdk::{
    Client,
    config::RequestConfig,
    ruma::{
        OwnedRoomId, RoomId, UInt, api::client::sync::sync_events::v5, events::StateEventType,
        presence::PresenceState,
    },
};
use serde::Serialize;
use serde_json::Value;

use crate::session::{self, DataDir};

/// The connection ID of the one-off requests, so they never touch the SDK's
/// `room-list` and `encryption` connections.
const CONN_ID: &str = "mxt-bridges";
const LIST: &str = "bridges";
const PAGE_SIZE: u32 = 100;
const SUBSCRIPTION_BATCH: usize = 100;
pub const M_BRIDGE: &str = "m.bridge";
pub const HALF_SHOT_BRIDGE: &str = "uk.half-shot.bridge";

#[derive(Clone, Debug, Serialize)]
pub struct BridgeEvent {
    pub event_type: String,
    pub state_key: String,
    pub network: Option<String>,
    pub protocol: Option<String>,
    /// False when the content is empty, which is how a bridge removes its state.
    pub has_content: bool,
}

/// Which step of the design's network name order produced the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NameStep {
    /// (1) `network.displayname` of `m.bridge` or `uk.half-shot.bridge`.
    BridgeNetwork,
    /// (2) `protocol.displayname` of that event.
    BridgeProtocol,
    /// (3) the canonical parent space (`m.space.parent` with `canonical: true`).
    CanonicalSpace,
    /// (4) the only joined parent space from `m.space.parent`.
    OnlyJoinedSpace,
    /// Not in the design: the only joined space that lists the room in its
    /// `m.space.child` state, for rooms without usable `m.space.parent`.
    OnlyJoinedSpaceByChild,
    /// (5) none.
    None,
}

impl NameStep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BridgeNetwork => "bridge-network",
            Self::BridgeProtocol => "bridge-protocol",
            Self::CanonicalSpace => "canonical-space",
            Self::OnlyJoinedSpace => "only-joined-space",
            Self::OnlyJoinedSpaceByChild => "only-joined-space-by-child",
            Self::None => "none",
        }
    }
}

/// The bridge state of every room the paged list and the follow-up
/// subscriptions returned. A room that is present with no events has no bridge.
#[derive(Default, Serialize)]
pub struct Scan {
    #[serde(skip)]
    pub rooms: BTreeMap<OwnedRoomId, Vec<BridgeEvent>>,
    pub list_count: u64,
    pub pages: usize,
    /// `fresh` when every page was a new request without `pos`, otherwise
    /// which page needed `pos` from the previous response.
    pub paging: String,
    pub notes: Vec<String>,
    pub duplicates: usize,
    pub rooms_from_list: usize,
    pub resubscribed: usize,
    pub resubscription_returned: usize,
    pub unchecked: Vec<OwnedRoomId>,
    pub elapsed_ms: u128,
}

impl Scan {
    /// Name and step from the bridge state alone (steps 1 and 2).
    pub fn network_name(&self, room_id: &RoomId) -> Option<(String, NameStep)> {
        let events = self.rooms.get(room_id)?;
        let ordered = || {
            events
                .iter()
                .filter(|e| e.event_type == M_BRIDGE)
                .chain(events.iter().filter(|e| e.event_type == HALF_SHOT_BRIDGE))
        };
        ordered()
            .find_map(|e| e.network.clone())
            .map(|name| (name, NameStep::BridgeNetwork))
            .or_else(|| {
                ordered()
                    .find_map(|e| e.protocol.clone())
                    .map(|name| (name, NameStep::BridgeProtocol))
            })
    }

    pub fn has_event(&self, room_id: &RoomId, event_type: &str) -> bool {
        self.rooms.get(room_id).is_some_and(|events| {
            events
                .iter()
                .any(|e| e.event_type == event_type && e.has_content)
        })
    }
}

fn required_state() -> Vec<(StateEventType, String)> {
    [M_BRIDGE, HALF_SHOT_BRIDGE]
        .into_iter()
        .map(|event_type| (StateEventType::from(event_type), "*".to_owned()))
        .collect()
}

fn base_request(pos: Option<String>) -> v5::Request {
    let mut request = v5::Request::new();
    request.conn_id = Some(CONN_ID.to_owned());
    request.pos = pos;
    // Answer immediately, and do not mark the user online.
    request.timeout = Some(Duration::ZERO);
    request.set_presence = PresenceState::Offline;
    request
}

fn list_request(first: u32, pos: Option<String>) -> v5::Request {
    let mut list = v5::request::List::default();
    list.ranges = vec![(UInt::from(first), UInt::from(first + PAGE_SIZE - 1))];
    list.room_details.required_state = required_state();
    list.room_details.timeline_limit = UInt::from(0u32);
    let mut request = base_request(pos);
    request.lists.insert(LIST.to_owned(), list);
    request
}

fn subscription_request(room_ids: &[OwnedRoomId]) -> v5::Request {
    let mut request = base_request(None);
    for room_id in room_ids {
        let mut subscription = v5::request::RoomSubscription::default();
        subscription.required_state = required_state();
        subscription.timeline_limit = UInt::from(0u32);
        request
            .room_subscriptions
            .insert(room_id.clone(), subscription);
    }
    request
}

fn request_config() -> RequestConfig {
    RequestConfig::new()
        .timeout(Duration::from_secs(120))
        .retry_limit(2)
}

fn parse_events(room: &v5::response::Room) -> Vec<BridgeEvent> {
    room.required_state
        .iter()
        .filter_map(|raw| serde_json::from_str::<Value>(raw.json().get()).ok())
        .filter_map(|event| {
            let event_type = event.get("type")?.as_str()?;
            if event_type != M_BRIDGE && event_type != HALF_SHOT_BRIDGE {
                return None;
            }
            let content = event.get("content");
            let display_name = |key: &str| {
                content
                    .and_then(|c| c.get(key))
                    .and_then(|v| v.get("displayname"))
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            };
            Some(BridgeEvent {
                event_type: event_type.to_owned(),
                state_key: event
                    .get("state_key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                network: display_name("network"),
                protocol: display_name("protocol"),
                has_content: content
                    .and_then(Value::as_object)
                    .is_some_and(|c| !c.is_empty()),
            })
        })
        .collect()
}

/// Pages a list over all rooms until its `count` is covered, then asks for the
/// `known` rooms the pages missed with `room_subscriptions`.
pub async fn scan(client: &Client, known: &[OwnedRoomId]) -> Result<Scan> {
    let start = Instant::now();
    let mut scan = Scan {
        paging: "fresh".to_owned(),
        ..Scan::default()
    };
    let mut chained = false;
    let mut pos: Option<String> = None;
    let mut first: u32 = 0;
    loop {
        let request_pos = if chained { pos.clone() } else { None };
        let response = match client
            .send(list_request(first, request_pos))
            .with_request_config(request_config())
            .await
        {
            Ok(response) => response,
            Err(error) if first > 0 && !chained => {
                scan.notes.push(format!(
                    "a fresh request for rooms {first}.. failed ({error}); retrying with pos"
                ));
                scan.paging = format!("pos chained from page {}", scan.pages + 1);
                chained = true;
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("bridge list request for {first}.."));
            }
        };
        scan.pages += 1;
        scan.list_count = response
            .lists
            .get(LIST)
            .map_or(0, |list| u64::from(list.count));
        if response.rooms.is_empty() && first > 0 && !chained && u64::from(first) < scan.list_count
        {
            scan.notes.push(format!(
                "a fresh request for rooms {first}.. returned no rooms; retrying with pos"
            ));
            scan.paging = format!("pos chained from page {}", scan.pages);
            chained = true;
            continue;
        }
        for (room_id, room) in &response.rooms {
            if scan
                .rooms
                .insert(room_id.clone(), parse_events(room))
                .is_some()
            {
                scan.duplicates += 1;
            }
        }
        pos = Some(response.pos);
        first += PAGE_SIZE;
        if u64::from(first) >= scan.list_count || scan.pages >= 1000 {
            break;
        }
    }
    scan.rooms_from_list = scan.rooms.len();

    let missing: Vec<OwnedRoomId> = known
        .iter()
        .filter(|room_id| !scan.rooms.contains_key(*room_id))
        .cloned()
        .collect();
    scan.resubscribed = missing.len();
    for batch in missing.chunks(SUBSCRIPTION_BATCH) {
        let response = client
            .send(subscription_request(batch))
            .with_request_config(request_config())
            .await
            .context("bridge room subscription request")?;
        for (room_id, room) in &response.rooms {
            scan.resubscription_returned += 1;
            scan.rooms.insert(room_id.clone(), parse_events(room));
        }
    }
    scan.unchecked = missing
        .into_iter()
        .filter(|room_id| !scan.rooms.contains_key(room_id))
        .collect();
    scan.elapsed_ms = start.elapsed().as_millis();
    Ok(scan)
}

/// `bridge-raw <room_id>`: the raw bridge state of one room.
pub async fn run_raw(dir: &DataDir, room_id: &RoomId) -> Result<()> {
    let session = session::restore(dir).await?;
    let response = session
        .client
        .send(subscription_request(&[room_id.to_owned()]))
        .with_request_config(request_config())
        .await
        .context("bridge room subscription request")?;
    let Some(room) = response.rooms.get(room_id) else {
        println!("the server returned no data for {room_id}");
        return Ok(());
    };
    if room.required_state.is_empty() {
        println!("{room_id} has no {M_BRIDGE} or {HALF_SHOT_BRIDGE} state");
    }
    for raw in &room.required_state {
        let event: Value = serde_json::from_str(raw.json().get())?;
        println!("{}", serde_json::to_string_pretty(&event)?);
    }
    Ok(())
}

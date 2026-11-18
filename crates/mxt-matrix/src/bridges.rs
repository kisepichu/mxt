//! Bridge state from one-off MSC4186 requests, and `m.bridge` parsing.
//!
//! The room list service's required state has no `m.bridge`, and a second
//! SDK sliding sync would feed its responses into the SDK's sync processing
//! (design, "Matrix adapter"). So the requests here go out with
//! `Client::send` on a connection of their own, and their responses are
//! parsed here and never reach the SDK's store or event cache.

use std::{collections::HashSet, sync::Arc, time::Duration};

use matrix_sdk::{
    Client,
    config::RequestConfig,
    ruma::{
        OwnedRoomId, UInt,
        api::client::sync::sync_events::v5,
        events::{AnySyncStateEvent, StateEventType},
        presence::PresenceState,
        serde::Raw,
    },
};
use matrix_sdk_ui::{RoomListService, room_list_service};
use mxt_core::room::BridgeInfo;
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, info, warn};

use crate::{Error, backoff::Backoff};

/// The connection ID of the one-off requests, so they never touch the SDK's
/// `room-list` and `encryption` connections.
const CONN_ID: &str = "mxt-bridges";
const LIST: &str = "bridges";
const PAGE_SIZE: u32 = 100;
/// 100 000 rooms; a guard against a server whose `count` never gets covered.
const MAX_PAGES: u32 = 1000;
const SUBSCRIPTION_BATCH: usize = 100;
/// Rooms whose check failed are asked for again after 1 min, doubling up to
/// 1 h while the checks keep failing.
const FIRST_RETRY: Duration = Duration::from_secs(60);
const MAX_RETRY: Duration = Duration::from_secs(60 * 60);
const M_BRIDGE: &str = "m.bridge";
const HALF_SHOT_BRIDGE: &str = "uk.half-shot.bridge";

/// Bridge state per room: the parsed information, or `None` when the room
/// has no bridge.
pub(crate) type BridgeStates = Vec<(OwnedRoomId, Option<BridgeInfo>)>;

/// Once the room list service runs, pages a list over all rooms, then asks
/// with `room_subscriptions` for every listed room (sent on `listed`) that no
/// page returned, and later for each room that appears. Each room is checked
/// once; the results go to `results`.
pub(crate) async fn run(
    client: Client,
    room_list: Arc<RoomListService>,
    mut listed: UnboundedReceiver<Vec<OwnedRoomId>>,
    results: UnboundedSender<BridgeStates>,
) {
    let mut pending = HashSet::new();
    let mut state = room_list.state();
    // Two responses have arrived, so the homeserver answers.
    while !matches!(state.get(), room_list_service::State::Running) {
        tokio::select! {
            changed = state.next() => if changed.is_none() { return },
            rooms = listed.recv() => match rooms {
                Some(rooms) => pending.extend(rooms),
                None => return,
            },
        }
    }

    let mut checked = HashSet::new();
    if let Err(error) = scan_list(&client, &results, &mut checked).await {
        warn!(
            "fetching the bridge state of the room list failed: {}",
            error.brief()
        );
        debug!("the bridge list scan failed: {error}");
    }

    let mut retry = Backoff::new(FIRST_RETRY, MAX_RETRY);
    loop {
        while let Ok(rooms) = listed.try_recv() {
            pending.extend(rooms);
        }
        pending.retain(|room| !checked.contains(room));
        let mut failed = Vec::new();
        let mut last_error = None;
        let rooms: Vec<OwnedRoomId> = pending.drain().collect();
        for batch in rooms.chunks(SUBSCRIPTION_BATCH) {
            match check_rooms(&client, batch).await {
                Ok(states) => {
                    // Rooms the server did not return are not asked for again.
                    checked.extend(batch.iter().cloned());
                    if results.send(states).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    failed.extend_from_slice(batch);
                    last_error = Some(error);
                }
            }
        }
        let retry_in = match last_error {
            None => {
                retry.reset();
                None
            }
            Some(error) => {
                let delay = retry.next_delay();
                warn!(
                    "fetching the bridge state of {} rooms failed ({}); asking again in {} s",
                    failed.len(),
                    error.brief(),
                    delay.as_secs()
                );
                debug!("the bridge room subscriptions failed: {error}");
                Some(delay)
            }
        };
        pending.extend(failed);
        tokio::select! {
            rooms = listed.recv() => match rooms {
                Some(rooms) => pending.extend(rooms),
                None => return,
            },
            () = tokio::time::sleep(retry_in.unwrap_or_default()), if retry_in.is_some() => {}
        }
    }
}

/// Pages the list until its `count` is covered and sends each page's rooms.
/// Pages are fresh requests without `pos`; when a later fresh page fails or
/// comes back empty below `count`, the rest is chained with `pos`.
async fn scan_list(
    client: &Client,
    results: &UnboundedSender<BridgeStates>,
    checked: &mut HashSet<OwnedRoomId>,
) -> Result<(), Error> {
    let mut chained = false;
    let mut pos = None;
    let mut first = 0;
    for _ in 0..MAX_PAGES {
        let request = list_request(first, if chained { pos.clone() } else { None });
        let response = match send(client, request).await {
            Ok(response) => response,
            Err(error) if first > 0 && !chained => {
                info!(
                    "a fresh bridge list page failed ({}); chaining pos from now on",
                    error.brief()
                );
                debug!("the fresh bridge list page failed: {error}");
                chained = true;
                continue;
            }
            Err(error) => return Err(error),
        };
        let count = response
            .lists
            .get(LIST)
            .map_or(0, |list| u64::from(list.count));
        if response.rooms.is_empty() && first > 0 && !chained && u64::from(first) < count {
            info!("a fresh bridge list page came back empty; chaining pos from now on");
            chained = true;
            continue;
        }
        debug!(
            first,
            count,
            rooms = response.rooms.len(),
            "bridge list page"
        );
        let _ = results.send(page_states(&response, checked));
        pos = Some(response.pos);
        first += PAGE_SIZE;
        if u64::from(first) >= count {
            break;
        }
    }
    Ok(())
}

async fn check_rooms(client: &Client, rooms: &[OwnedRoomId]) -> Result<BridgeStates, Error> {
    let response = send(client, subscription_request(rooms)).await?;
    debug!(
        asked = rooms.len(),
        returned = response.rooms.len(),
        "bridge room subscriptions"
    );
    Ok(states(&response))
}

async fn send(client: &Client, request: v5::Request) -> Result<v5::Response, Error> {
    let config = RequestConfig::new()
        .timeout(Duration::from_secs(120))
        .retry_limit(2);
    Ok(client.send(request).with_request_config(config).await?)
}

fn states(response: &v5::Response) -> BridgeStates {
    response.rooms.iter().map(room_state).collect()
}

/// The bridge state of a list page's rooms that no earlier page returned,
/// or that the server sends complete (`initial`). With chained `pos`, a room
/// the connection has sent before comes back with only the state that
/// changed, and an empty `required_state` would read as "no bridge".
fn page_states(response: &v5::Response, checked: &mut HashSet<OwnedRoomId>) -> BridgeStates {
    response
        .rooms
        .iter()
        .filter(|(room, data)| checked.insert((*room).clone()) || data.initial == Some(true))
        .map(room_state)
        .collect()
}

fn room_state(
    (room, data): (&OwnedRoomId, &v5::response::Room),
) -> (OwnedRoomId, Option<BridgeInfo>) {
    (room.clone(), room_bridge_state(&data.required_state))
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
    // Answer at once, and do not mark the user online.
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

fn subscription_request(rooms: &[OwnedRoomId]) -> v5::Request {
    let mut request = base_request(None);
    for room in rooms {
        let mut subscription = v5::request::RoomSubscription::default();
        subscription.required_state = required_state();
        subscription.timeline_limit = UInt::from(0u32);
        request
            .room_subscriptions
            .insert(room.clone(), subscription);
    }
    request
}

fn room_bridge_state(state: &[Raw<AnySyncStateEvent>]) -> Option<BridgeInfo> {
    let events: Vec<(String, Value)> = state
        .iter()
        .filter_map(|raw| {
            let event_type = raw.get_field::<String>("type").ok()??;
            let content = raw.get_field::<Value>("content").ok()??;
            Some((event_type, content))
        })
        .collect();
    room_bridge(
        events
            .iter()
            .map(|(event_type, content)| (event_type.as_str(), content)),
    )
}

/// One room's bridge information from its bridge state events: the first
/// network and the first protocol display name, `m.bridge` events before
/// `uk.half-shot.bridge` ones. `None` when no event has content; a bridge
/// removes its state by emptying it.
fn room_bridge<'a>(events: impl Iterator<Item = (&'a str, &'a Value)>) -> Option<BridgeInfo> {
    let mut bridges: Vec<(bool, BridgeInfo)> = events
        .filter(|(_, content)| content.as_object().is_some_and(|c| !c.is_empty()))
        .filter_map(|(event_type, content)| match event_type {
            M_BRIDGE => Some((false, parse_bridge(content))),
            HALF_SHOT_BRIDGE => Some((true, parse_bridge(content))),
            _ => None,
        })
        .collect();
    if bridges.is_empty() {
        return None;
    }
    // Stable, so events of one type keep the server's order.
    bridges.sort_by_key(|(half_shot, _)| *half_shot);
    let first = |field: fn(&BridgeInfo) -> &Option<String>| {
        bridges.iter().find_map(|(_, info)| field(info).clone())
    };
    Some(BridgeInfo {
        network: first(|info| &info.network),
        protocol: first(|info| &info.protocol),
    })
}

/// `content.network.displayname` and `content.protocol.displayname` of an
/// `m.bridge` or `uk.half-shot.bridge` state event's content. Blank names
/// count as missing.
pub(crate) fn parse_bridge(content: &Value) -> BridgeInfo {
    let display_name = |key: &str| {
        content
            .get(key)?
            .get("displayname")?
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
    };
    BridgeInfo {
        network: display_name("network"),
        protocol: display_name("protocol"),
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::owned_room_id;
    use serde_json::json;

    use super::*;

    fn info(network: Option<&str>, protocol: Option<&str>) -> BridgeInfo {
        BridgeInfo {
            network: network.map(Into::into),
            protocol: protocol.map(Into::into),
        }
    }

    #[test]
    fn parse_bridge_reads_network_and_protocol_display_names() {
        let content = json!({
            "bridgebot": "@discordbot:example.org",
            "protocol": { "id": "discord", "displayname": "Discord" },
            "network": { "id": "123", "displayname": "myserver" },
            "channel": { "id": "456", "displayname": "general" },
        });
        assert_eq!(
            parse_bridge(&content),
            info(Some("myserver"), Some("Discord"))
        );
    }

    #[test]
    fn parse_bridge_without_network() {
        let content = json!({ "protocol": { "id": "discord", "displayname": "Discord" } });
        assert_eq!(parse_bridge(&content), info(None, Some("Discord")));
    }

    #[test]
    fn parse_bridge_without_display_names() {
        let content = json!({
            "protocol": { "id": "telegram" },
            "network": { "id": "123", "displayname": 7 },
        });
        assert_eq!(parse_bridge(&content), info(None, None));
    }

    #[test]
    fn parse_bridge_of_non_object_content() {
        assert_eq!(parse_bridge(&json!("m.bridge")), info(None, None));
        assert_eq!(parse_bridge(&json!(null)), info(None, None));
        assert_eq!(
            parse_bridge(&json!({ "network": "myserver" })),
            info(None, None)
        );
    }

    #[test]
    fn room_bridge_prefers_m_bridge_and_ignores_emptied_state() {
        let half_shot = json!({ "network": { "displayname": "half-shot network" } });
        let m_bridge = json!({ "protocol": { "displayname": "Discord" } });
        let removed = json!({});
        let events = [
            (HALF_SHOT_BRIDGE, &half_shot),
            (M_BRIDGE, &removed),
            (M_BRIDGE, &m_bridge),
        ];
        assert_eq!(
            room_bridge(events.into_iter()),
            Some(info(Some("half-shot network"), Some("Discord")))
        );
        let m_bridge_network = json!({ "network": { "displayname": "myserver" } });
        let events = [
            (HALF_SHOT_BRIDGE, &half_shot),
            (M_BRIDGE, &m_bridge_network),
        ];
        assert_eq!(
            room_bridge(events.into_iter()),
            Some(info(Some("myserver"), None))
        );
        assert_eq!(room_bridge([(M_BRIDGE, &removed)].into_iter()), None);
    }

    /// A list page's room with at most one state event.
    fn page_room(initial: Option<bool>, state: Option<&Value>) -> v5::response::Room {
        let mut room = v5::response::Room::new();
        room.initial = initial;
        room.required_state = state
            .into_iter()
            .map(|event| Raw::from_json(serde_json::value::to_raw_value(event).unwrap()))
            .collect();
        room
    }

    #[test]
    fn a_room_from_an_earlier_page_counts_only_when_sent_complete() {
        let m_bridge = json!({
            "type": M_BRIDGE,
            "state_key": "bridge",
            "event_id": "$bridge:example.org",
            "sender": "@bot:example.org",
            "origin_server_ts": 1,
            "content": { "network": { "displayname": "myserver" } },
        });
        let (a, b, c) = (
            owned_room_id!("!a:example.org"),
            owned_room_id!("!b:example.org"),
            owned_room_id!("!c:example.org"),
        );
        let mut checked = HashSet::new();
        let mut first = v5::Response::new("1".to_owned());
        first
            .rooms
            .insert(a.clone(), page_room(None, Some(&m_bridge)));
        first
            .rooms
            .insert(c.clone(), page_room(None, Some(&m_bridge)));
        let bridged = Some(info(Some("myserver"), None));
        assert_eq!(
            page_states(&first, &mut checked),
            vec![(a.clone(), bridged.clone()), (c.clone(), bridged.clone())]
        );

        // A chained page: `a` only with what changed, `b` new, `c` complete.
        let mut chained = v5::Response::new("2".to_owned());
        chained.rooms.insert(a, page_room(None, None));
        chained.rooms.insert(b.clone(), page_room(None, None));
        chained
            .rooms
            .insert(c.clone(), page_room(Some(true), Some(&m_bridge)));
        assert_eq!(
            page_states(&chained, &mut checked),
            vec![(b, None), (c, bridged)]
        );
    }
}

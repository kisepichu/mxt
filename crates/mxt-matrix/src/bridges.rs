//! Bridge state from the stored room state, and `m.bridge` parsing.

use matrix_sdk::{
    Room, deserialized_responses::RawAnySyncOrStrippedState, ruma::events::StateEventType,
};
use mxt_core::room::BridgeInfo;
use serde_json::Value;

use crate::Error;

const M_BRIDGE: &str = "m.bridge";
const HALF_SHOT_BRIDGE: &str = "uk.half-shot.bridge";

/// The room's bridge information from its `m.bridge` and
/// `uk.half-shot.bridge` state, which classic sync delivers in full; `None`
/// when the room has no bridge.
pub(crate) async fn bridge_info(room: &Room) -> Result<Option<BridgeInfo>, Error> {
    let mut events = Vec::new();
    for event_type in [M_BRIDGE, HALF_SHOT_BRIDGE] {
        for raw in room
            .get_state_events(StateEventType::from(event_type))
            .await?
        {
            if let Some(content) = content(&raw) {
                events.push((event_type, content));
            }
        }
    }
    Ok(room_bridge(
        events
            .iter()
            .map(|(event_type, content)| (*event_type, content)),
    ))
}

fn content(raw: &RawAnySyncOrStrippedState) -> Option<Value> {
    match raw {
        RawAnySyncOrStrippedState::Sync(raw) => raw.get_field("content"),
        RawAnySyncOrStrippedState::Stripped(raw) => raw.get_field("content"),
    }
    .ok()
    .flatten()
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
    // Stable, so events of one type keep the store's order.
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
fn parse_bridge(content: &Value) -> BridgeInfo {
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
}

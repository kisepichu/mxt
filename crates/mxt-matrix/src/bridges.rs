//! Bridge state from the stored room state, and `m.bridge` parsing.

use matrix_sdk::{
    Room,
    deserialized_responses::RawAnySyncOrStrippedState,
    ruma::{OwnedUserId, UserId, events::StateEventType},
};
use mxt_core::room::BridgeInfo;
use serde_json::Value;

use crate::Error;

const M_BRIDGE: &str = "m.bridge";
const HALF_SHOT_BRIDGE: &str = "uk.half-shot.bridge";

/// What a room's bridge state says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Bridge {
    pub info: BridgeInfo,
    /// The bridge's bot (`bridgebot`), which is a member of the rooms it
    /// bridges, direct messages included.
    pub bot: Option<OwnedUserId>,
}

/// The room's bridge from its `m.bridge` and `uk.half-shot.bridge` state,
/// which classic sync delivers in full; `None` when the room has no bridge.
pub(crate) async fn bridge(room: &Room) -> Result<Option<Bridge>, Error> {
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

/// One room's bridge from its bridge state events: the first network and
/// the first protocol display name, and the first bridge bot, `m.bridge`
/// events before `uk.half-shot.bridge` ones. `None` when no event has
/// content; a bridge removes its state by emptying it.
fn room_bridge<'a>(events: impl Iterator<Item = (&'a str, &'a Value)>) -> Option<Bridge> {
    let mut bridges: Vec<(bool, Bridge)> = events
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
        bridges
            .iter()
            .find_map(|(_, bridge)| field(&bridge.info).clone())
    };
    Some(Bridge {
        info: BridgeInfo {
            network: first(|info| &info.network),
            protocol: first(|info| &info.protocol),
        },
        bot: bridges.iter().find_map(|(_, bridge)| bridge.bot.clone()),
    })
}

/// `content.network.displayname`, `content.protocol.displayname` and
/// `content.bridgebot` of an `m.bridge` or `uk.half-shot.bridge` state
/// event's content. Blank names and invalid user IDs count as missing.
fn parse_bridge(content: &Value) -> Bridge {
    let display_name = |key: &str| {
        content
            .get(key)?
            .get("displayname")?
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
    };
    Bridge {
        info: BridgeInfo {
            network: display_name("network"),
            protocol: display_name("protocol"),
        },
        bot: content
            .get("bridgebot")
            .and_then(Value::as_str)
            .and_then(|bot| UserId::parse(bot).ok()),
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::owned_user_id;
    use serde_json::json;

    use super::*;

    fn info(network: Option<&str>, protocol: Option<&str>) -> Bridge {
        Bridge {
            info: BridgeInfo {
                network: network.map(Into::into),
                protocol: protocol.map(Into::into),
            },
            bot: None,
        }
    }

    #[test]
    fn parse_bridge_reads_network_and_protocol_display_names_and_the_bot() {
        let content = json!({
            "bridgebot": "@discordbot:example.org",
            "protocol": { "id": "discord", "displayname": "Discord" },
            "network": { "id": "123", "displayname": "myserver" },
            "channel": { "id": "456", "displayname": "general" },
        });
        assert_eq!(
            parse_bridge(&content),
            Bridge {
                bot: Some(owned_user_id!("@discordbot:example.org")),
                ..info(Some("myserver"), Some("Discord"))
            }
        );
    }

    #[test]
    fn parse_bridge_without_network() {
        let content = json!({ "protocol": { "id": "discord", "displayname": "Discord" } });
        assert_eq!(parse_bridge(&content), info(None, Some("Discord")));
    }

    #[test]
    fn parse_bridge_without_display_names_or_a_valid_bot() {
        let content = json!({
            "bridgebot": "discordbot",
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
        let half_shot = json!({
            "bridgebot": "@halfshotbot:example.org",
            "network": { "displayname": "half-shot network" },
        });
        let m_bridge = json!({
            "bridgebot": "@discordbot:example.org",
            "protocol": { "displayname": "Discord" },
        });
        let removed = json!({});
        let events = [
            (HALF_SHOT_BRIDGE, &half_shot),
            (M_BRIDGE, &removed),
            (M_BRIDGE, &m_bridge),
        ];
        assert_eq!(
            room_bridge(events.into_iter()),
            Some(Bridge {
                bot: Some(owned_user_id!("@discordbot:example.org")),
                ..info(Some("half-shot network"), Some("Discord"))
            })
        );
        let m_bridge_network = json!({ "network": { "displayname": "myserver" } });
        let events = [
            (HALF_SHOT_BRIDGE, &half_shot),
            (M_BRIDGE, &m_bridge_network),
        ];
        assert_eq!(
            room_bridge(events.into_iter()),
            Some(Bridge {
                bot: Some(owned_user_id!("@halfshotbot:example.org")),
                ..info(Some("myserver"), None)
            })
        );
        assert_eq!(room_bridge([(M_BRIDGE, &removed)].into_iter()), None);
    }
}

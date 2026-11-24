//! `SetMode` (design, "Notification modes"): the requests of each target on
//! a room with a room rule without actions, as mautrix-discord puts on a
//! guild channel, on a muted room and on a room without a rule; none when
//! the room has the mode already; and a refused change answered with the
//! SDK's mode before the error.

use matrix_sdk::{ruma::push::RuleKind, test_utils::mocks::PushRuleIdSpec};
use serde_json::{Value, json};
use wiremock::ResponseTemplate;

use super::*;

const RULES: &str = "/_matrix/client/v3/pushrules/global/";
/// A room the client does not know: a change of it fails without a request.
const UNKNOWN: &str = "!unknown:example.org";

/// `ROOM`'s room rule with `actions`.
fn room_rule(actions: Value) -> Value {
    json!({ "rule_id": ROOM, "default": false, "enabled": true, "actions": actions })
}

/// The override rule that mutes `ROOM`, as `set_room_notification_mode`
/// puts it.
fn mute_rule() -> Value {
    json!({
        "rule_id": ROOM,
        "default": false,
        "enabled": true,
        "conditions": [{ "kind": "event_match", "key": "room_id", "pattern": ROOM }],
        "actions": [],
    })
}

/// The actions of a rule that notifies, with the default sound.
fn notifying() -> Value {
    json!(["notify", { "set_tweak": "sound", "value": "default" }])
}

impl Adapter {
    /// The adapter of a client that has joined `ROOM`, with the account's
    /// push rules `room` and `override` synced as global account data.
    async fn with_rules(room: Vec<Value>, overrides: Vec<Value>) -> Adapter {
        let adapter = Adapter::without_rooms().await;
        let rules = json!({
            "type": "m.push_rules",
            "content": { "global": { "room": room, "override": overrides } },
        });
        let room_id = ruma::RoomId::parse(ROOM).unwrap();
        adapter
            .server
            .mock_sync()
            .ok_and_run(&adapter.client, |builder| {
                builder.add_joined_room(JoinedRoomBuilder::new(&room_id));
                builder.add_custom_global_account_data(rules);
            })
            .await;
        adapter
    }

    fn set_mode(&self, room: &str, mode: NotificationMode) {
        self.effect(MatrixEffect::SetMode {
            room: RoomId::new(room),
            mode,
        });
    }

    /// Waits until the changes sent before have run: a change of a room the
    /// client does not know runs after them in the same task and fails
    /// without a request, answered with its mode and the error. Whatever
    /// the changes before sent the core comes first.
    async fn changes_done(&mut self) {
        self.set_mode(UNKNOWN, NotificationMode::AllMessages);
        let unknown = Msg::RoomModeChanged {
            room: RoomId::new(UNKNOWN),
            mode: NotificationMode::Default,
        };
        assert_eq!(self.core.report().await, unknown);
        assert!(matches!(self.core.report().await, Msg::EffectFailed { .. }));
    }

    /// The bodies of the `PUT` requests of `ROOM`'s rule of `kind`.
    async fn puts(&self, kind: &str) -> Vec<Value> {
        let path = format!("{RULES}{kind}/{ROOM}");
        let requests = self.requests(RULES).await;
        requests
            .iter()
            .filter(|request| request.method.as_str() == "PUT" && request.url.path() == path)
            .map(|request| request.body_json().unwrap())
            .collect()
    }
}

#[tokio::test]
async fn all_messages_replaces_a_room_rule_without_actions() {
    let mut adapter = Adapter::with_rules(vec![room_rule(json!([]))], vec![]).await;
    let server = &adapter.server;
    let room_rule = PushRuleIdSpec::Some(ROOM);
    server
        .mock_set_push_rules(RuleKind::Room, room_rule)
        .ok()
        .expect(1)
        .mount()
        .await;
    for kind in [RuleKind::Room, RuleKind::Override] {
        let any = PushRuleIdSpec::Any;
        server
            .mock_delete_push_rules(kind, any)
            .ok()
            .expect(0)
            .mount()
            .await;
    }
    adapter.set_mode(ROOM, NotificationMode::AllMessages);
    // A success sends the core nothing: the sync's echo of the rules does.
    adapter.changes_done().await;
    assert_eq!(
        adapter.puts("room").await,
        [json!({ "actions": notifying() })]
    );
    adapter.server.verify_and_reset().await;
}

#[tokio::test]
async fn a_room_that_has_the_mode_already_makes_no_request() {
    let mut adapter = Adapter::with_rules(vec![room_rule(notifying())], vec![]).await;
    for kind in [RuleKind::Room, RuleKind::Override] {
        let server = &adapter.server;
        let set = server.mock_set_push_rules(kind.clone(), PushRuleIdSpec::Any);
        set.ok().expect(0).mount().await;
        let delete = server.mock_delete_push_rules(kind, PushRuleIdSpec::Any);
        delete.ok().expect(0).mount().await;
    }
    adapter.set_mode(ROOM, NotificationMode::AllMessages);
    adapter.changes_done().await;
    adapter.server.verify_and_reset().await;
}

#[tokio::test]
async fn all_messages_on_a_muted_room_puts_a_room_rule_and_deletes_the_override_rule() {
    let mut adapter = Adapter::with_rules(vec![], vec![mute_rule()]).await;
    let server = &adapter.server;
    let room_rule = PushRuleIdSpec::Some(ROOM);
    server
        .mock_set_push_rules(RuleKind::Room, room_rule)
        .ok()
        .expect(1)
        .mount()
        .await;
    let override_rule = PushRuleIdSpec::Some(ROOM);
    server
        .mock_delete_push_rules(RuleKind::Override, override_rule)
        .ok()
        .expect(1)
        .mount()
        .await;
    adapter.set_mode(ROOM, NotificationMode::AllMessages);
    adapter.changes_done().await;
    assert_eq!(
        adapter.puts("room").await,
        [json!({ "actions": notifying() })]
    );
    adapter.server.verify_and_reset().await;
}

#[tokio::test]
async fn mentions_on_a_room_without_a_rule_puts_a_room_rule_without_actions() {
    let mut adapter = Adapter::with_rules(vec![], vec![]).await;
    let server = &adapter.server;
    let room_rule = PushRuleIdSpec::Some(ROOM);
    server
        .mock_set_push_rules(RuleKind::Room, room_rule)
        .ok()
        .expect(1)
        .mount()
        .await;
    for kind in [RuleKind::Room, RuleKind::Override] {
        let any = PushRuleIdSpec::Any;
        server
            .mock_delete_push_rules(kind, any)
            .ok()
            .expect(0)
            .mount()
            .await;
    }
    adapter.set_mode(ROOM, NotificationMode::MentionsAndKeywords);
    adapter.changes_done().await;
    assert_eq!(adapter.puts("room").await, [json!({ "actions": [] })]);
    adapter.server.verify_and_reset().await;
}

#[tokio::test]
async fn default_on_a_muted_room_only_deletes_the_override_rule() {
    let mut adapter = Adapter::with_rules(vec![], vec![mute_rule()]).await;
    let server = &adapter.server;
    let override_rule = PushRuleIdSpec::Some(ROOM);
    server
        .mock_delete_push_rules(RuleKind::Override, override_rule)
        .ok()
        .expect(1)
        .mount()
        .await;
    for kind in [RuleKind::Room, RuleKind::Override] {
        let any = PushRuleIdSpec::Any;
        server
            .mock_set_push_rules(kind, any)
            .ok()
            .expect(0)
            .mount()
            .await;
    }
    adapter.set_mode(ROOM, NotificationMode::Default);
    adapter.changes_done().await;
    adapter.server.verify_and_reset().await;
}

#[tokio::test]
async fn a_refused_change_puts_the_sdks_mode_back_before_the_error() {
    let mut adapter = Adapter::with_rules(vec![room_rule(json!([]))], vec![]).await;
    // Permanent: the SDK retries a 5xx, not a 403.
    let forbidden = ResponseTemplate::new(403).set_body_json(json!({
        "errcode": "M_FORBIDDEN",
        "error": "Forbidden",
    }));
    adapter
        .server
        .mock_set_push_rules(RuleKind::Room, PushRuleIdSpec::Some(ROOM))
        .respond_with(forbidden)
        .expect(1)
        .mount()
        .await;
    adapter.set_mode(ROOM, NotificationMode::AllMessages);
    let unchanged = Msg::RoomModeChanged {
        room: room(),
        mode: NotificationMode::MentionsAndKeywords,
    };
    assert_eq!(adapter.core.report().await, unchanged);
    match adapter.core.report().await {
        Msg::EffectFailed { error } => assert!(
            error.starts_with("Changing the room's notifications failed: "),
            "{error}"
        ),
        other => panic!("not the failure: {other:?}"),
    }
    adapter.server.verify_and_reset().await;
}

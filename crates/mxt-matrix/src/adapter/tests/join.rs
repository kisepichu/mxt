//! `:join`: room targets and the direct message lookup (design, "Joining
//! rooms and direct messages"). `/join` by ID or alias and the `m.direct`
//! account data have no mock in `MatrixMockServer`, so plain wiremock mocks
//! answer them; every test that must not create a room says so with
//! `mock_create_room().expect(0)`.

use matrix_sdk::ruma::{
    OwnedRoomId, OwnedUserId, RoomVersionId,
    events::{AnyStrippedStateEvent, AnySyncStateEvent, room::member::MembershipState},
    serde::Raw,
};
use matrix_sdk_test::{InvitedRoomBuilder, LeftRoomBuilder};
use mxt_core::{msg::JoinedHow, room::Invite};
use serde_json::{Value, json};
use wiremock::{
    Mock, Request, ResponseTemplate,
    matchers::{method, path_regex},
};

use super::*;
use crate::rooms::Rooms;

const BOB: &str = "@bob:example.org";
const BOT: &str = "@discordbot:example.org";
/// A Discord user, as the bridge's puppet.
const PERSON: &str = "@discord_1:example.org";
/// `/join/{roomIdOrAlias}`.
const JOIN_PATH: &str = "/_matrix/client/v3/join/";

fn room_id(id: &str) -> OwnedRoomId {
    ruma::RoomId::parse(id).unwrap()
}

fn user_id(id: &str) -> OwnedUserId {
    ruma::UserId::parse(id).unwrap()
}

fn join(target: &str) -> MatrixEffect {
    MatrixEffect::Join {
        target: target.to_owned(),
    }
}

fn create_dm(user: &str) -> MatrixEffect {
    MatrixEffect::CreateDm {
        user: UserId::new(user),
    }
}

fn joined(target: &str, room: &str, how: JoinedHow) -> Msg {
    Msg::Joined {
        target: target.to_owned(),
        room: RoomId::new(room),
        how,
    }
}

fn no_dm(user: &str) -> Msg {
    Msg::NoDirectMessage {
        target: user.to_owned(),
        user: UserId::new(user),
    }
}

fn failed(error: &str) -> Msg {
    Msg::EffectFailed {
        error: error.to_owned(),
    }
}

/// A joined room whose `members` (the user among them) have joined, with
/// the server's member counts.
fn room_with(id: &str, members: &[&str], invited: u64) -> JoinedRoomBuilder {
    let f = EventFactory::new();
    let room = JoinedRoomBuilder::new(&room_id(id)).set_room_summary(json!({
        "m.joined_member_count": members.len(),
        "m.invited_member_count": invited,
    }));
    members.iter().fold(room, |room, member| {
        room.add_state_event(f.member(&user_id(member)))
    })
}

/// A state event built from JSON, for the bridge state, which has no type
/// in ruma.
fn state(event_type: &str, state_key: &str, content: Value) -> Raw<AnySyncStateEvent> {
    Raw::new(&json!({
        "type": event_type,
        "state_key": state_key,
        "sender": BOT,
        "event_id": format!("$state-{event_type}:example.org"),
        "origin_server_ts": 1,
        "content": content,
    }))
    .unwrap()
    .cast_unchecked()
}

/// `m.bridge` state whose bot is the Discord bridge bot.
fn bridge(extra: Value) -> Raw<AnySyncStateEvent> {
    let mut content = json!({
        "bridgebot": BOT,
        "protocol": { "id": "discord", "displayname": "Discord" },
    });
    content
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    state("m.bridge", "discord", content)
}

/// A pending invite to `room` from `inviter`, direct or not.
fn invite(room: &str, inviter: &str, is_direct: bool) -> InvitedRoomBuilder {
    let member: Raw<AnyStrippedStateEvent> = Raw::new(&json!({
        "type": "m.room.member",
        "state_key": ME,
        "sender": inviter,
        "content": { "membership": "invite", "is_direct": is_direct },
    }))
    .unwrap()
    .cast_unchecked();
    InvitedRoomBuilder::new(&room_id(room)).add_state_event(member)
}

impl Adapter {
    /// The answer to `effect`.
    async fn answer(&mut self, effect: MatrixEffect) -> Msg {
        self.effect(effect);
        self.core.report().await
    }

    /// `/join` by ID or alias, answered with `room`, expected `times`.
    async fn mock_join(&self, room: &str, times: u64) {
        Mock::given(method("POST"))
            .and(path_regex(format!("^{JOIN_PATH}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "room_id": room })))
            .expect(times)
            .mount(self.server.server())
            .await;
    }

    /// The `m.room.create` state request with `content`, expected once.
    async fn mock_create_event(&self, content: Value) {
        Mock::given(method("GET"))
            .and(path_regex(
                "^/_matrix/client/v3/rooms/[^/]+/state/m\\.room\\.create/?$",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(content))
            .expect(1)
            .mount(self.server.server())
            .await;
    }

    /// No room is created: the server checks it when the test ends.
    async fn creates_nothing(&self) {
        self.server.mock_create_room().ok().expect(0).mount().await;
    }

    /// The requests the server received whose path starts with `prefix`.
    async fn requests(&self, prefix: &str) -> Vec<Request> {
        let requests = self.server.server().received_requests().await.unwrap();
        requests
            .into_iter()
            .filter(|request| request.url.path().starts_with(prefix))
            .collect()
    }

    /// The `via` servers of each `/join` request.
    async fn join_via(&self) -> Vec<Vec<String>> {
        let requests = self.requests(JOIN_PATH).await;
        requests
            .iter()
            .map(|request| {
                let pairs = request.url.query_pairs();
                pairs
                    .filter(|(key, _)| key == "via")
                    .map(|(_, value)| value.into_owned())
                    .collect()
            })
            .collect()
    }
}

#[tokio::test]
async fn an_alias_is_resolved_and_joined() {
    let mut adapter = Adapter::without_rooms().await;
    adapter
        .server
        .mock_room_directory_resolve_alias()
        .ok("!mxt:example.org", vec!["example.org".to_owned()])
        .expect(1)
        .mount()
        .await;
    adapter.mock_join("!mxt:example.org", 1).await;
    // The store has not seen the room: one request tells it is no space.
    adapter
        .mock_create_event(json!({ "room_version": "11" }))
        .await;
    assert_eq!(
        adapter.answer(join("#mxt:example.org")).await,
        joined("#mxt:example.org", "!mxt:example.org", JoinedHow::Joined)
    );
    // The homeserver resolves the alias: no `via`.
    assert_eq!(adapter.join_via().await, [Vec::<String>::new()]);
}

#[tokio::test]
async fn a_room_id_with_a_pending_invite_is_joined_through_its_server() {
    let mut adapter = Adapter::without_rooms().await;
    let room = "!invited:example.org";
    let invited = invite(room, "@carol:other.example", false);
    adapter.server.sync_room(&adapter.client, invited).await;
    adapter.mock_join(room, 1).await;
    assert_eq!(
        adapter.answer(join(room)).await,
        joined(room, room, JoinedHow::Joined)
    );
    assert_eq!(adapter.join_via().await, [["example.org"]]);
}

#[tokio::test]
async fn a_link_s_via_reaches_the_request() {
    let mut adapter = Adapter::without_rooms().await;
    let room = "!linked:example.org";
    adapter.mock_join(room, 1).await;
    // Percent-encoded, with a message.
    let link = "https://matrix.to/#/%21linked%3Aexample.org/%24event%3Aexample.org\
                ?via=other.example&via=third.example";
    assert_eq!(
        adapter.answer(join(link)).await,
        joined(link, room, JoinedHow::Joined)
    );
    assert_eq!(
        adapter.join_via().await,
        [["other.example", "third.example"]]
    );
}

#[tokio::test]
async fn a_left_room_is_joined_through_its_creators_server() {
    let mut adapter = Adapter::without_rooms().await;
    let room = "!left:example.org";
    let create = EventFactory::new().create(&user_id("@carol:creator.example"), RoomVersionId::V11);
    let left = LeftRoomBuilder::new(&room_id(room)).add_state_bulk([create.into()]);
    adapter.server.sync_room(&adapter.client, left).await;
    adapter.mock_join(room, 1).await;
    assert_eq!(
        adapter.answer(join(room)).await,
        joined(room, room, JoinedHow::Joined)
    );
    assert_eq!(
        adapter.join_via().await,
        [["example.org", "creator.example"]]
    );
}

#[tokio::test]
async fn an_unknown_alias_and_a_refused_join_say_why() {
    let mut adapter = Adapter::without_rooms().await;
    adapter
        .server
        .mock_room_directory_resolve_alias()
        .not_found()
        .mount()
        .await;
    assert_eq!(
        adapter.answer(join("#nowhere:example.org")).await,
        failed("Joining failed: no room has this alias")
    );

    Mock::given(method("POST"))
        .and(path_regex(format!("^{JOIN_PATH}")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "errcode": "M_FORBIDDEN",
            "error": "You are not invited to !closed:example.org",
        })))
        .mount(adapter.server.server())
        .await;
    assert_eq!(
        adapter.answer(join("!closed:example.org")).await,
        failed("Joining failed: the homeserver answered 403 M_FORBIDDEN")
    );
    assert_eq!(
        adapter.answer(join("nonsense")).await,
        failed("Joining failed: not a room alias, room ID, user ID or matrix.to link")
    );
}

#[tokio::test]
async fn a_joined_room_is_answered_by_its_id_or_aliases_without_a_request() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.mock_join("!mxt:example.org", 0).await;
    adapter
        .server
        .mock_room_directory_resolve_alias()
        .ok("!mxt:example.org", Vec::new())
        .expect(0)
        .mount()
        .await;
    let aliases = EventFactory::new().sender(&user_id(ME)).canonical_alias(
        Some("#mxt:example.org".try_into().unwrap()),
        vec!["#mxt-old:example.org".try_into().unwrap()],
    );
    let room = room_with("!mxt:example.org", &[ME], 0).add_state_event(aliases);
    adapter.server.sync_room(&adapter.client, room).await;
    for target in [
        "!mxt:example.org",
        "#mxt:example.org",
        "#mxt-old:example.org",
    ] {
        assert_eq!(
            adapter.answer(join(target)).await,
            joined(target, "!mxt:example.org", JoinedHow::Already)
        );
    }
}

#[tokio::test]
async fn a_joined_space_is_an_error_without_a_request() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.mock_join("!space:example.org", 0).await;
    let create = EventFactory::new()
        .create(&user_id(ME), RoomVersionId::V11)
        .with_space_type();
    let space = room_with("!space:example.org", &[ME], 0).add_state_event(create);
    adapter.server.sync_room(&adapter.client, space).await;
    assert_eq!(
        adapter.answer(join("!space:example.org")).await,
        failed("The room is a space, which mxt does not list")
    );
}

#[tokio::test]
async fn a_space_the_join_entered_is_an_error() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.mock_join("!space:example.org", 1).await;
    adapter
        .mock_create_event(json!({ "room_version": "11", "type": "m.space" }))
        .await;
    assert_eq!(
        adapter.answer(join("!space:example.org")).await,
        failed("Joined, but the room is a space, which mxt does not list")
    );
}

#[tokio::test]
async fn a_room_in_m_direct_is_opened() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    // A third member: only `m.direct` finds it.
    let room = room_with("!dm:example.org", &[ME, BOB, "@carol:example.org"], 0);
    let direct = EventFactory::new()
        .direct()
        .add_user(user_id(BOB).into(), &room_id("!dm:example.org"));
    adapter
        .server
        .mock_sync()
        .ok_and_run(&adapter.client, |sync| {
            sync.add_joined_room(room).add_global_account_data(direct);
        })
        .await;
    assert_eq!(
        adapter.answer(join(BOB)).await,
        joined(BOB, "!dm:example.org", JoinedHow::Already)
    );
}

#[tokio::test]
async fn a_room_in_m_direct_that_the_user_left_is_skipped() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    let f = EventFactory::new();
    let left = f.member(&user_id(BOB)).membership(MembershipState::Leave);
    let room = room_with("!dm:example.org", &[ME], 0).add_state_event(left);
    let direct = f
        .direct()
        .add_user(user_id(BOB).into(), &room_id("!dm:example.org"));
    adapter
        .server
        .mock_sync()
        .ok_and_run(&adapter.client, |sync| {
            sync.add_joined_room(room).add_global_account_data(direct);
        })
        .await;
    assert_eq!(adapter.answer(join(BOB)).await, no_dm(BOB));
}

#[tokio::test]
async fn a_bridge_bots_room_is_found_with_or_without_bridge_state() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    let plain = room_with("!bot:example.org", &[ME, BOT], 0);
    adapter.server.sync_room(&adapter.client, plain).await;
    assert_eq!(
        adapter.answer(join(BOT)).await,
        joined(BOT, "!bot:example.org", JoinedHow::Already)
    );

    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    let bridged = room_with("!bot:example.org", &[ME, BOT], 0).add_state_event(bridge(json!({})));
    adapter.server.sync_room(&adapter.client, bridged).await;
    assert_eq!(
        adapter.answer(join(BOT)).await,
        joined(BOT, "!bot:example.org", JoinedHow::Already)
    );
}

#[tokio::test]
async fn a_named_bridged_channel_with_only_the_bot_is_not_its_room() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    let name = EventFactory::new()
        .sender(&user_id(BOT))
        .room_name("general");
    let channel = room_with("!general:example.org", &[ME, BOT], 0)
        .add_state_event(bridge(json!({})))
        .add_state_event(name);
    adapter.server.sync_room(&adapter.client, channel).await;
    assert_eq!(adapter.answer(join(BOT)).await, no_dm(BOT));
}

/// A room with the user's member event only, whose heroes are Bob.
fn with_hero_bob(invited: u64) -> JoinedRoomBuilder {
    room_with("!dm:example.org", &[ME], invited).set_room_summary(json!({
        "m.heroes": [BOB],
        "m.joined_member_count": 2,
        "m.invited_member_count": invited,
    }))
}

#[tokio::test]
async fn heroes_find_a_room_without_a_members_request() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    adapter
        .server
        .mock_get_members()
        .ok(Vec::new())
        .expect(0)
        .mount()
        .await;
    adapter
        .server
        .sync_room(&adapter.client, with_hero_bob(0))
        .await;
    assert_eq!(
        adapter.answer(join(BOB)).await,
        joined(BOB, "!dm:example.org", JoinedHow::Already)
    );
}

#[tokio::test]
async fn with_an_invited_member_one_members_request_decides() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    let f = EventFactory::new().room(&room_id("!dm:example.org"));
    let members = [ME, BOB, "@carol:example.org"].map(|member| {
        let membership = if member == BOB || member == ME {
            MembershipState::Join
        } else {
            MembershipState::Invite
        };
        f.member(&user_id(member)).membership(membership).into_raw()
    });
    adapter
        .server
        .mock_get_members()
        .ok(members.to_vec())
        .expect(1)
        .mount()
        .await;
    adapter
        .server
        .sync_room(&adapter.client, with_hero_bob(1))
        .await;
    assert_eq!(
        adapter.answer(join(BOB)).await,
        joined(BOB, "!dm:example.org", JoinedHow::Already)
    );
}

#[tokio::test]
async fn two_member_rooms_with_someone_else_are_passed_over_without_a_request() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    adapter
        .server
        .mock_get_members()
        .ok(Vec::new())
        .expect(0)
        .mount()
        .await;
    // The store has Carol as the other joined member.
    let with_carol = room_with("!carol:example.org", &[ME, "@carol:example.org"], 0);
    adapter.server.sync_room(&adapter.client, with_carol).await;
    // The store has the user's member event only; the heroes are Dave and
    // nobody is invited.
    let with_dave = room_with("!dave:example.org", &[ME], 0).set_room_summary(json!({
        "m.heroes": ["@dave:example.org"],
        "m.joined_member_count": 2,
        "m.invited_member_count": 0,
    }));
    adapter.server.sync_room(&adapter.client, with_dave).await;
    assert_eq!(adapter.answer(join(BOB)).await, no_dm(BOB));
}

#[tokio::test]
async fn a_bridged_direct_message_is_found_by_the_persons_id() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    let room = room_with("!bridged:example.org", &[ME, BOT, PERSON], 0)
        .add_state_event(bridge(json!({ "com.beeper.room_type": "dm" })));
    adapter.server.sync_room(&adapter.client, room).await;
    assert_eq!(
        adapter.answer(join(PERSON)).await,
        joined(PERSON, "!bridged:example.org", JoinedHow::Already)
    );
}

#[tokio::test]
async fn a_pending_direct_invite_from_the_user_is_accepted() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    let room = "!invite:example.org";
    adapter
        .server
        .sync_room(&adapter.client, invite(room, BOB, true))
        .await;
    adapter
        .server
        .mock_room_join(&room_id(room))
        .ok()
        .expect(1)
        .mount()
        .await;
    // The SDK adds an accepted direct invite to `m.direct`, with the
    // room's members.
    let f = EventFactory::new().room(&room_id(room));
    let members = [ME, BOB].map(|member| f.member(&user_id(member)).into_raw());
    adapter
        .server
        .mock_get_members()
        .ok(members.to_vec())
        .mount()
        .await;
    mock_m_direct(&adapter).await;
    assert_eq!(
        adapter.answer(join(BOB)).await,
        joined(BOB, room, JoinedHow::Joined)
    );
}

#[tokio::test]
async fn nothing_found_is_said_and_the_own_id_is_an_error() {
    let mut adapter = Adapter::without_rooms().await;
    adapter.creates_nothing().await;
    // An invite from Bob that is not direct does not count.
    let invited = invite("!party:example.org", BOB, false);
    adapter.server.sync_room(&adapter.client, invited).await;
    assert_eq!(adapter.answer(join(BOB)).await, no_dm(BOB));
    let link = format!("https://matrix.to/#/{BOB}");
    assert_eq!(
        adapter.answer(join(&link)).await,
        Msg::NoDirectMessage {
            target: link,
            user: UserId::new(BOB),
        }
    );
    assert_eq!(
        adapter.answer(join(ME)).await,
        failed("Opening the direct message failed: that is your own user ID")
    );
}

/// `m.direct`: none on the server yet, and the write.
async fn mock_m_direct(adapter: &Adapter) {
    let m_direct = "^/_matrix/client/v3/user/.*/account_data/m\\.direct$";
    Mock::given(method("GET"))
        .and(path_regex(m_direct))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "errcode": "M_NOT_FOUND",
            "error": "Account data not found",
        })))
        .mount(adapter.server.server())
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(m_direct))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(adapter.server.server())
        .await;
}

#[tokio::test]
async fn create_dm_creates_an_encrypted_direct_message_when_none_is_found() {
    let mut adapter = Adapter::without_rooms().await;
    adapter
        .server
        .mock_create_room()
        .ok()
        .expect(1)
        .mount()
        .await;
    mock_m_direct(&adapter).await;
    // `mock_create_room` answers with this room.
    let created = "!room:example.org";
    assert_eq!(
        adapter.answer(create_dm(BOB)).await,
        joined(BOB, created, JoinedHow::Created)
    );

    let requests = adapter.requests("/_matrix/client/v3/createRoom").await;
    let [request] = requests.as_slice() else {
        panic!("{requests:?}");
    };
    let body: Value = request.body_json().unwrap();
    assert_eq!(body["is_direct"], json!(true));
    assert_eq!(body["invite"], json!([BOB]));
    assert_eq!(body["preset"], json!("trusted_private_chat"));
    let initial_state = body["initial_state"].as_array().unwrap();
    assert!(
        initial_state
            .iter()
            .any(|event| event["type"] == "m.room.encryption"),
        "{body}"
    );
    let writes = adapter.requests("/_matrix/client/v3/user/").await;
    let write = writes
        .iter()
        .find(|request| request.method.as_str() == "PUT");
    let m_direct: Value = write.unwrap().body_json().unwrap();
    let mut expected = serde_json::Map::new();
    expected.insert(BOB.to_owned(), json!([created]));
    assert_eq!(m_direct, Value::Object(expected));
}

#[tokio::test]
async fn two_quick_create_dms_and_a_join_share_one_new_room() {
    let mut adapter = Adapter::without_rooms().await;
    adapter
        .server
        .mock_create_room()
        .ok()
        .expect(1)
        .mount()
        .await;
    mock_m_direct(&adapter).await;
    let created = "!room:example.org";
    adapter.effect(create_dm(BOB));
    adapter.effect(create_dm(BOB));
    let mut answers = vec![adapter.core.report().await, adapter.core.report().await];
    answers.sort_by_key(|answer| format!("{answer:?}"));
    assert_eq!(
        answers,
        [
            joined(BOB, created, JoinedHow::Already),
            joined(BOB, created, JoinedHow::Created),
        ]
    );
    // Before any sync brings `m.direct` back.
    assert_eq!(
        adapter.answer(join(BOB)).await,
        joined(BOB, created, JoinedHow::Already)
    );
}

#[tokio::test]
async fn pending_invites_reach_the_core_with_their_names_when_they_change() {
    let adapter = Adapter::without_rooms().await;
    let (to_core, mut from_rooms) = mpsc::unbounded_channel();
    let settings = adapter.client.notification_settings().await;
    let mut rooms = Rooms::new(adapter.client.clone(), settings, to_core);
    rooms.check_invites().await;
    assert_eq!(from_rooms.try_recv(), Ok(Msg::Invites(Vec::new())));

    let named = |name: &str| {
        let event = EventFactory::new().sender(&user_id(BOB)).room_name(name);
        invite("!book:example.org", BOB, false).add_state_event(event)
    };
    let book_club = |name: &str| {
        Msg::Invites(vec![Invite {
            room: RoomId::new("!book:example.org"),
            name: name.to_owned(),
        }])
    };
    adapter
        .server
        .sync_room(&adapter.client, named("Book club"))
        .await;
    rooms.check_invites().await;
    assert_eq!(from_rooms.try_recv(), Ok(book_club("Book club")));
    rooms.check_invites().await;
    assert!(from_rooms.try_recv().is_err(), "unchanged");

    adapter
        .server
        .sync_room(&adapter.client, named("Reading club"))
        .await;
    rooms.check_invites().await;
    assert_eq!(from_rooms.try_recv(), Ok(book_club("Reading club")));
}

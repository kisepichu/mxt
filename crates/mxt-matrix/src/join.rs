//! `:join` (design, "Joining rooms and direct messages"): joining a room by
//! its alias, its ID or a matrix.to link, which also accepts its invite, and
//! finding the direct message with a user, or creating one after the user's
//! `y`.

use std::{cmp::Reverse, collections::HashMap, sync::Arc};

use matrix_sdk::{
    Client, Room, RoomState,
    ruma::{
        MatrixToUri, OwnedRoomId, OwnedRoomOrAliasId, OwnedServerName, OwnedUserId, RoomId,
        RoomOrAliasId, UserId, api::error::ErrorKind, events::room::member::MembershipState,
        matrix_uri::MatrixId,
    },
};
use mxt_core::{
    ids,
    msg::{JoinedHow, Msg},
};
use tokio::sync::{Mutex, mpsc::UnboundedSender};
use tracing::info;

use crate::{
    Error,
    adapter::report,
    bridges::{self, Bridge, RoomType},
    error::without_url,
    rooms::bridged_dm_partner,
};

/// The rooms `Client::create_dm` returned this session, per user. The SDK
/// writes `m.direct` to the server, but its store learns of the new room
/// only when a later sync brings the account data back, so the lookup's
/// first step reads these too. Each lookup of a user, with its creation,
/// holds the lock, so two of them never create two rooms.
#[derive(Clone, Default)]
pub(crate) struct DirectMessages(Arc<Mutex<HashMap<OwnedUserId, OwnedRoomId>>>);

/// `MatrixEffect::Join`: joins the room `target` names, or finds the
/// direct message with the user it names; answers `Msg::Joined`,
/// `Msg::NoDirectMessage` or a failure.
pub(crate) async fn join(
    client: Client,
    dms: DirectMessages,
    target: String,
    to_core: UnboundedSender<Msg>,
) {
    let answer = match parse(&target) {
        None => Err(Failure::Join(Error::NotAJoinTarget)),
        Some(Target::Room { id, via }) => join_room(&client, &target, &id, via).await,
        Some(Target::User(user)) => direct_message(&client, &dms, &target, &user, false).await,
    };
    answered(&to_core, answer);
}

/// `MatrixEffect::CreateDm`: looks once more, and creates the direct
/// message only when it still finds none; the answer's target is the user
/// ID.
pub(crate) async fn create_dm(
    client: Client,
    dms: DirectMessages,
    user: ids::UserId,
    to_core: UnboundedSender<Msg>,
) {
    let target = user.to_string();
    let answer = match UserId::parse(user.as_str()) {
        Ok(user) => direct_message(&client, &dms, &target, &user, true).await,
        Err(_) => Err(Failure::DirectMessage(Error::NotAJoinTarget)),
    };
    answered(&to_core, answer);
}

/// What a target names.
enum Target {
    /// A room ID or alias, and the servers of a link's `via`.
    Room {
        id: OwnedRoomOrAliasId,
        via: Vec<OwnedServerName>,
    },
    User(OwnedUserId),
}

/// A matrix.to link (to an event: its room), a user ID, or a room ID or
/// alias, as ruma reads them.
fn parse(target: &str) -> Option<Target> {
    if let Ok(link) = MatrixToUri::parse(target) {
        let via = link.via().to_vec();
        return match link.id() {
            MatrixId::Room(id) => Some(Target::Room {
                id: id.clone().into(),
                via,
            }),
            MatrixId::RoomAlias(alias) => Some(Target::Room {
                id: alias.clone().into(),
                via,
            }),
            MatrixId::Event(id, _) => Some(Target::Room {
                id: id.clone(),
                via,
            }),
            MatrixId::User(user) => Some(Target::User(user.clone())),
            _ => None,
        };
    }
    if let Ok(user) = UserId::parse(target) {
        return Some(Target::User(user));
    }
    let id = RoomOrAliasId::parse(target).ok()?;
    Some(Target::Room {
        id,
        via: Vec::new(),
    })
}

/// Why a `:join` or a `y` failed.
enum Failure {
    /// mxt does not list spaces; `joined` when the join just entered one.
    Space { joined: bool },
    /// Joining a room target.
    Join(Error),
    /// Finding or creating a direct message.
    DirectMessage(Error),
}

/// Sends the answer, or the failure's status, which names neither the
/// target nor the room.
fn answered(to_core: &UnboundedSender<Msg>, answer: Result<Msg, Failure>) {
    let status = match answer {
        Ok(answer) => {
            let _ = to_core.send(answer);
            return;
        }
        Err(Failure::Join(error)) => return report(to_core, "Joining failed", &error),
        Err(Failure::DirectMessage(error)) => {
            return report(to_core, "Opening the direct message failed", &error);
        }
        Err(Failure::Space { joined: false }) => "The room is a space, which mxt does not list",
        Err(Failure::Space { joined: true }) => {
            "Joined, but the room is a space, which mxt does not list"
        }
    };
    let _ = to_core.send(Msg::EffectFailed {
        error: status.to_owned(),
    });
}

fn joined(target: &str, room: &Room, how: JoinedHow) -> Msg {
    Msg::Joined {
        target: target.to_owned(),
        room: ids::RoomId::new(room.room_id().as_str()),
        how,
    }
}

/// A joined room by its ID, canonical alias or alternative alias is
/// answered without a request; otherwise `/join`, which accepts an invite,
/// through the link's `via` servers or those of `fallback_via`.
async fn join_room(
    client: &Client,
    target: &str,
    id: &RoomOrAliasId,
    via: Vec<OwnedServerName>,
) -> Result<Msg, Failure> {
    if let Some(room) = joined_room(client, id) {
        if room.is_space() {
            return Err(Failure::Space { joined: false });
        }
        return Ok(joined(target, &room, JoinedHow::Already));
    }
    let via = if via.is_empty() {
        fallback_via(client, id)
    } else {
        via
    };
    let room = client
        .join_room_by_id_or_alias(id, &via)
        .await
        .map_err(|error| {
            let unknown_alias = id.is_room_alias_id()
                && error.client_api_error_kind() == Some(&ErrorKind::NotFound);
            Failure::Join(if unknown_alias {
                Error::UnknownAlias
            } else {
                without_url(error).into()
            })
        })?;
    if room.is_space() {
        return Err(Failure::Space { joined: true });
    }
    Ok(joined(target, &room, JoinedHow::Joined))
}

fn joined_room(client: &Client, id: &RoomOrAliasId) -> Option<Room> {
    match <&RoomId>::try_from(id) {
        Ok(id) => client
            .get_room(id)
            .filter(|room| room.state() == RoomState::Joined),
        Err(alias) => client.joined_rooms().into_iter().find(|room| {
            room.canonical_alias().as_deref() == Some(alias)
                || room.alt_aliases().iter().any(|alt| alt == alias)
        }),
    }
}

/// For a room ID: its server name, when it has one, and the servers of the
/// room's creators when the store still has the room (left, or invited). A
/// homeserver that left a room joins it again only through a server in it,
/// and newer room IDs have no server name.
fn fallback_via(client: &Client, id: &RoomOrAliasId) -> Vec<OwnedServerName> {
    let Ok(id) = <&RoomId>::try_from(id) else {
        return Vec::new();
    };
    let mut via: Vec<OwnedServerName> = id
        .server_name()
        .map(ToOwned::to_owned)
        .into_iter()
        .collect();
    let creators = client
        .get_room(id)
        .and_then(|room| room.creators())
        .unwrap_or_default();
    for creator in creators {
        let server = creator.server_name().to_owned();
        if !via.contains(&server) {
            via.push(server);
        }
    }
    via
}

/// The room a step of the lookup found.
struct Found {
    room: Room,
    step: u8,
}

/// Opens the direct message with `user`: the lookup's room, answered
/// `Already`, or a pending direct invite, accepted and answered `Joined`.
/// Without one, `Msg::NoDirectMessage`, or with `create` a new encrypted
/// direct message, answered `Created`.
async fn direct_message(
    client: &Client,
    dms: &DirectMessages,
    target: &str,
    user: &UserId,
    create: bool,
) -> Result<Msg, Failure> {
    if client.user_id() == Some(user) {
        return Err(Failure::DirectMessage(Error::OwnUserId));
    }
    let failed = |error: matrix_sdk::Error| Failure::DirectMessage(without_url(error).into());
    let mut created = dms.0.lock().await;
    let found = lookup(client, &created, user)
        .await
        .map_err(Failure::DirectMessage)?;
    match found {
        Some(Found { room, step }) => {
            info!("direct message lookup: step {step} found a room");
            if room.state() == RoomState::Invited {
                room.join().await.map_err(failed)?;
                return Ok(joined(target, &room, JoinedHow::Joined));
            }
            Ok(joined(target, &room, JoinedHow::Already))
        }
        None => {
            info!("direct message lookup: no step found a room");
            if !create {
                return Ok(Msg::NoDirectMessage {
                    target: target.to_owned(),
                    user: ids::UserId::new(user.as_str()),
                });
            }
            let room = client.create_dm(user).await.map_err(failed)?;
            created.insert(user.to_owned(), room.room_id().to_owned());
            Ok(joined(target, &room, JoinedHow::Created))
        }
    }
}

/// The design's lookup, in order; the first step that finds a room decides,
/// and within a step the room with the latest activity. Members are
/// lazy-loaded, so only step 2 may send a request: one `/members` for a
/// room that neither the store nor the heroes decide.
async fn lookup(
    client: &Client,
    created: &HashMap<OwnedUserId, OwnedRoomId>,
    user: &UserId,
) -> Result<Option<Found>, Error> {
    let found = |room: Option<Room>, step| room.map(|room| Found { room, step });

    // 1. Direct messages: the rooms this session created for the user and
    //    the joined rooms whose `m.direct` targets are exactly the user,
    //    unless the store has the user as left or banned there.
    let mut direct: Vec<Room> = created
        .get(user)
        .and_then(|room| client.get_room(room))
        .filter(|room| room.state() == RoomState::Joined)
        .into_iter()
        .collect();
    for room in client.get_dm_rooms(user) {
        if direct.iter().all(|other| other.room_id() != room.room_id()) {
            direct.push(room);
        }
    }
    let mut still_there = Vec::new();
    for room in direct {
        if !has_left(&room, user).await? {
            still_there.push(room);
        }
    }
    if let Some(found) = found(latest(still_there), 1) {
        return Ok(Some(found));
    }

    // The joined rooms that are not spaces, newest first, with their bridge
    // state, for steps 2 and 3.
    let mut rooms = Vec::new();
    for room in client.joined_rooms() {
        if !room.is_space() {
            let bridge = bridges::bridge(&room).await?;
            rooms.push((room, bridge));
        }
    }
    rooms.sort_by_key(|(room, _)| Reverse(room.latest_event_timestamp()));

    // 2. A room the user shares with that user alone, without bridge state
    //    unless it is the bridge bot's own room.
    for (room, bridge) in &rooms {
        if room.joined_members_count() == 2
            && bridge
                .as_ref()
                .is_none_or(|bridge| is_bots_room(room, bridge, user))
            && is_joined_member(room, user).await?
        {
            return Ok(found(Some(room.clone()), 2));
        }
    }

    // 3. A bridged direct message with that user, by the room list's rule.
    for (room, bridge) in &rooms {
        let Some(bridge) = bridge else {
            continue;
        };
        if bridged_dm_partner(room, bridge)
            .await?
            .is_some_and(|partner| partner.id == user)
        {
            return Ok(found(Some(room.clone()), 3));
        }
    }

    // 4. A pending invite from that user to a room marked direct.
    let mut invites = Vec::new();
    for room in client.invited_rooms() {
        if room.is_direct().await? && room.invite_details().await?.inviter_id == user {
            invites.push(room);
        }
    }
    Ok(found(latest(invites), 4))
}

/// The room with the latest activity.
fn latest(rooms: Vec<Room>) -> Option<Room> {
    rooms
        .into_iter()
        .max_by_key(|room| room.latest_event_timestamp())
}

/// Whether the store has `user`'s membership in `room` as left or banned;
/// a room without their member event stays.
async fn has_left(room: &Room, user: &UserId) -> Result<bool, Error> {
    let member = room.get_member_no_sync(user).await?;
    Ok(member.is_some_and(|member| {
        matches!(
            member.membership(),
            MembershipState::Leave | MembershipState::Ban
        )
    }))
}

/// A bridged room counts in step 2 only as the bridge bot's own room: the
/// user is the bot, the room has no `m.room.name`, and the bridge declares
/// no room type or `dm`. mautrix names every channel it bridges, so a
/// channel that only the user and the bot have joined is not taken for it.
fn is_bots_room(room: &Room, bridge: &Bridge, user: &UserId) -> bool {
    bridge.bot.as_deref() == Some(user)
        && room.name().is_none()
        && matches!(bridge.room_type, None | Some(RoomType::Dm))
}

/// Whether `user` is a joined member of a room with two joined members:
/// the store decides when it has their member event; otherwise the heroes
/// when they are exactly that user and nobody is invited (heroes list
/// invited members too); otherwise one `/members` request.
async fn is_joined_member(room: &Room, user: &UserId) -> Result<bool, Error> {
    let joined = |membership: &MembershipState| *membership == MembershipState::Join;
    if let Some(member) = room.get_member_no_sync(user).await? {
        return Ok(joined(member.membership()));
    }
    if room.invited_members_count() == 0
        && let [hero] = room.heroes().await.as_slice()
        && hero.user_id == user
    {
        return Ok(true);
    }
    let member = room
        .get_member(user)
        .await
        .map_err(|error| Error::from(without_url(error)))?;
    Ok(member.is_some_and(|member| joined(member.membership())))
}

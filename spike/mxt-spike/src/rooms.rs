//! `rooms [--wait SECS] [--json PATH]`: questions (1) sync path, (2) bridge
//! state and network names, (3) unread counts.

use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use matrix_sdk::{
    Client, Room, RoomState,
    deserialized_responses::SyncOrStrippedState,
    notification_settings::RoomNotificationMode,
    ruma::{OwnedRoomId, events::space::parent::SpaceParentEventContent},
};
use matrix_sdk_ui::{
    room_list_service::{self, RoomListLoadingState},
    spaces::SpaceService,
};
use serde::Serialize;

use crate::{
    bridge::{self, NameStep, Scan},
    session::{self, DataDir},
    util,
};

#[derive(Serialize)]
struct RoomRow {
    room_id: OwnedRoomId,
    kind: &'static str,
    invited: bool,
    network: Option<String>,
    network_step: NameStep,
    /// The room part of the displayed name: the other user's display name for
    /// direct messages, the room's display name otherwise.
    room_part: String,
    sdk_display_name: String,
    unread_messages: u64,
    unread_notifications: u64,
    unread_mentions: u64,
    marked_unread: bool,
    /// The server's push-rule based counts, for comparison.
    server_notifications: u64,
    server_highlights: u64,
    /// The user-defined notification mode; `None` means the account default.
    notification_mode: Option<&'static str>,
    latest_event: Option<String>,
    recency_stamp: Option<u64>,
    m_bridge: bool,
    half_shot_bridge: bool,
    bridge_state_checked: bool,
    space_parents: usize,
    joined_space_parents: usize,
    joined_spaces_listing_as_child: usize,
}

#[derive(Serialize)]
struct Totals {
    sliding_sync_version: String,
    rooms_in_store_at_start: usize,
    cached_maximum_number_of_rooms: Option<u32>,
    maximum_number_of_rooms: Option<u32>,
    first_response_ms: Option<u128>,
    loaded_ms: Option<u128>,
    rooms: usize,
    spaces: usize,
    direct_messages: usize,
    with_m_bridge: usize,
    with_half_shot_bridge: usize,
    by_step: BTreeMap<&'static str, usize>,
    named_from_spaces: usize,
    without_network_name: usize,
    bridge_scan: Scan,
}

#[derive(Serialize)]
struct Output {
    totals: Totals,
    rooms: Vec<RoomRow>,
}

pub async fn run(dir: &DataDir, wait: Duration, json: Option<&Path>) -> Result<()> {
    let session = session::restore(dir).await?;
    let client = &session.client;
    let rooms_in_store_at_start = client.joined_rooms().len() + client.invited_rooms().len();

    let start = Instant::now();
    let sync = session::start_sync(client).await?;
    let room_list = sync.room_list_service();
    let all_rooms = room_list.all_rooms().await?;
    let loading = all_rooms.loading_state();
    let list_state = room_list.state();
    let cached_maximum_number_of_rooms = match loading.get() {
        RoomListLoadingState::Loaded {
            maximum_number_of_rooms,
        } => maximum_number_of_rooms,
        RoomListLoadingState::NotLoaded => None,
    };

    let mut first_response_ms = None;
    let mut maximum_number_of_rooms = None;
    let loaded_ms = loop {
        session::check_sync(&sync)?;
        let state = list_state.get();
        if first_response_ms.is_none() && !matches!(state, room_list_service::State::Init) {
            first_response_ms = Some(start.elapsed().as_millis());
        }
        if let RoomListLoadingState::Loaded {
            maximum_number_of_rooms: max,
        } = loading.get()
        {
            maximum_number_of_rooms = max;
        }
        let count = client.joined_rooms().len() + client.invited_rooms().len();
        if matches!(state, room_list_service::State::Running)
            && maximum_number_of_rooms.is_some_and(|max| count >= max as usize)
        {
            break Some(start.elapsed().as_millis());
        }
        if start.elapsed() >= wait {
            eprintln!(
                "warning: the room list was not loaded within {wait:?} \
                 (state {state:?}, {count} of {maximum_number_of_rooms:?} rooms)"
            );
            break None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let mut rooms: Vec<Room> = client.joined_rooms();
    rooms.extend(client.invited_rooms());
    let known: Vec<OwnedRoomId> = rooms.iter().map(|r| r.room_id().to_owned()).collect();
    let scan = bridge::scan(client, &known).await?;

    let spaces = SpaceService::new(client.clone()).await;
    let notification_settings = client.notification_settings().await;
    let mut rows = Vec::with_capacity(rooms.len());
    for room in &rooms {
        rows.push(room_row(client, room, &scan, &spaces, &notification_settings).await?);
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.recency_stamp));
    sync.stop().await;

    let listed = || rows.iter().filter(|r| r.kind != "space");
    let mut by_step = BTreeMap::new();
    for row in listed() {
        *by_step.entry(row.network_step.as_str()).or_insert(0) += 1;
    }
    let totals = Totals {
        sliding_sync_version: format!("{:?}", client.sliding_sync_version()),
        rooms_in_store_at_start,
        cached_maximum_number_of_rooms,
        maximum_number_of_rooms,
        first_response_ms,
        loaded_ms,
        rooms: listed().count(),
        spaces: rows.iter().filter(|r| r.kind == "space").count(),
        direct_messages: listed().filter(|r| r.kind == "dm").count(),
        with_m_bridge: listed().filter(|r| r.m_bridge).count(),
        with_half_shot_bridge: listed().filter(|r| r.half_shot_bridge).count(),
        named_from_spaces: listed()
            .filter(|r| {
                matches!(
                    r.network_step,
                    NameStep::CanonicalSpace
                        | NameStep::OnlyJoinedSpace
                        | NameStep::OnlyJoinedSpaceByChild
                )
            })
            .count(),
        without_network_name: listed()
            .filter(|r| r.network_step == NameStep::None)
            .count(),
        by_step,
        bridge_scan: scan,
    };

    for row in &rows {
        print_row(row);
    }
    print_totals(&totals);

    if let Some(path) = json {
        let output = Output {
            totals,
            rooms: rows,
        };
        session::write_new_private(path, &serde_json::to_vec_pretty(&output)?)
            .context("writing --json (the file must not exist yet)")?;
        println!("json written to {}", path.display());
    }
    Ok(())
}

async fn room_row(
    client: &Client,
    room: &Room,
    scan: &Scan,
    spaces: &SpaceService,
    notification_settings: &matrix_sdk::notification_settings::NotificationSettings,
) -> Result<RoomRow> {
    let room_id = room.room_id();
    let is_space = room.is_space();
    let is_dm = room.is_direct().await?;
    let sdk_display_name = room.display_name().await?.to_string();
    let room_part = if is_dm {
        dm_partner_name(room)
            .await?
            .unwrap_or_else(|| sdk_display_name.clone())
    } else {
        sdk_display_name.clone()
    };

    // Parent spaces from the room's own `m.space.parent` state.
    let mut parents = Vec::new();
    for raw in room
        .get_state_events_static::<SpaceParentEventContent>()
        .await?
    {
        match raw.deserialize() {
            Ok(SyncOrStrippedState::Sync(event)) => {
                if let Some(original) = event.as_original() {
                    parents.push((original.state_key.clone(), original.content.canonical));
                }
            }
            Ok(SyncOrStrippedState::Stripped(event)) => {
                parents.push((event.state_key.clone(), event.content.canonical));
            }
            Err(_) => {}
        }
    }
    let mut joined_parents = Vec::new();
    for (parent_id, canonical) in &parents {
        if let Some(parent) = client.get_room(parent_id)
            && parent.state() == RoomState::Joined
            && parent.is_space()
        {
            joined_parents.push((parent.display_name().await?.to_string(), *canonical));
        }
    }
    let child_of = spaces.joined_parent_ids_of_child(room_id).await;

    let (network, network_step) = if let Some((name, step)) = scan.network_name(room_id) {
        (Some(name), step)
    } else if let Some((name, _)) = joined_parents.iter().find(|(_, canonical)| *canonical) {
        (Some(name.clone()), NameStep::CanonicalSpace)
    } else if let [(name, _)] = joined_parents.as_slice() {
        (Some(name.clone()), NameStep::OnlyJoinedSpace)
    } else if let ([], [space_id]) = (joined_parents.as_slice(), child_of.as_slice()) {
        let name = match client.get_room(space_id) {
            Some(space) => space.display_name().await?.to_string(),
            None => space_id.to_string(),
        };
        (Some(name), NameStep::OnlyJoinedSpaceByChild)
    } else {
        (None, NameStep::None)
    };

    let mode = notification_settings
        .get_user_defined_room_notification_mode(room_id)
        .await
        .map(|mode| match mode {
            RoomNotificationMode::AllMessages => "all-messages",
            RoomNotificationMode::MentionsAndKeywordsOnly => "mentions-and-keywords",
            RoomNotificationMode::Mute => "mute",
        });
    let server = room.unread_notification_counts();

    Ok(RoomRow {
        room_id: room_id.to_owned(),
        kind: if is_space {
            "space"
        } else if is_dm {
            "dm"
        } else {
            "group"
        },
        invited: room.state() == RoomState::Invited,
        network,
        network_step,
        room_part,
        sdk_display_name,
        unread_messages: room.num_unread_messages(),
        unread_notifications: room.num_unread_notifications(),
        unread_mentions: room.num_unread_mentions(),
        marked_unread: room.is_marked_unread(),
        server_notifications: server.notification_count,
        server_highlights: server.highlight_count,
        notification_mode: mode,
        latest_event: room.latest_event_timestamp().map(util::format_ts),
        recency_stamp: room.recency_stamp().map(u64::from),
        m_bridge: scan.has_event(room_id, bridge::M_BRIDGE),
        half_shot_bridge: scan.has_event(room_id, bridge::HALF_SHOT_BRIDGE),
        bridge_state_checked: scan.rooms.contains_key(room_id),
        space_parents: parents.len(),
        joined_space_parents: joined_parents.len(),
        joined_spaces_listing_as_child: child_of.len(),
    })
}

/// The display name of the other user of a direct message, from the members
/// the SDK already has (no `/members` request).
async fn dm_partner_name(room: &Room) -> Result<Option<String>> {
    let own = room.own_user_id();
    for target in room.direct_targets() {
        let Some(user_id) = target.as_user_id() else {
            continue;
        };
        if user_id == own {
            continue;
        }
        if let Some(member) = room.get_member_no_sync(user_id).await? {
            return Ok(Some(member.name().to_owned()));
        }
    }
    Ok(None)
}

fn print_row(row: &RoomRow) {
    let name = match &row.network {
        Some(network) => format!("{network} / {}", row.room_part),
        None => row.room_part.clone(),
    };
    println!(
        "{} {}{} \"{}\" step={} unread={}/{}/@{} marked={} server={}/{} mode={} latest={} bridge={}{}{} parents={}/{} child-of={}",
        row.room_id,
        row.kind,
        if row.invited { "(invite)" } else { "" },
        name,
        row.network_step.as_str(),
        row.unread_messages,
        row.unread_notifications,
        row.unread_mentions,
        row.marked_unread,
        row.server_notifications,
        row.server_highlights,
        row.notification_mode.unwrap_or("default"),
        row.latest_event.as_deref().unwrap_or("-"),
        if row.m_bridge { "m.bridge " } else { "" },
        if row.half_shot_bridge {
            "uk.half-shot.bridge "
        } else {
            ""
        },
        if row.bridge_state_checked {
            "checked"
        } else {
            "unchecked"
        },
        row.joined_space_parents,
        row.space_parents,
        row.joined_spaces_listing_as_child,
    );
}

fn print_totals(totals: &Totals) {
    let ms = |v: Option<u128>| v.map_or_else(|| "-".to_owned(), |ms| format!("{ms} ms"));
    let scan = &totals.bridge_scan;
    println!();
    println!("sliding sync version: {}", totals.sliding_sync_version);
    println!(
        "rooms in the store at start: {} (cached maximum_number_of_rooms {:?})",
        totals.rooms_in_store_at_start, totals.cached_maximum_number_of_rooms
    );
    println!(
        "first sync response after {}, room list loaded after {} (maximum_number_of_rooms {:?})",
        ms(totals.first_response_ms),
        ms(totals.loaded_ms),
        totals.maximum_number_of_rooms
    );
    println!(
        "rooms: {} ({} direct messages), spaces: {}",
        totals.rooms, totals.direct_messages, totals.spaces
    );
    println!(
        "rooms with m.bridge: {}, with uk.half-shot.bridge: {}",
        totals.with_m_bridge, totals.with_half_shot_bridge
    );
    println!(
        "named from spaces: {}, without a network name: {}",
        totals.named_from_spaces, totals.without_network_name
    );
    for (step, count) in &totals.by_step {
        println!("  step {step}: {count}");
    }
    println!(
        "bridge state: list count {}, {} pages, paging {}, {} rooms from the list, {} duplicates, \
         {} re-checked with room_subscriptions ({} returned), {} unchecked, {} ms",
        scan.list_count,
        scan.pages,
        scan.paging,
        scan.rooms_from_list,
        scan.duplicates,
        scan.resubscribed,
        scan.resubscription_returned,
        scan.unchecked.len(),
        scan.elapsed_ms
    );
    for note in &scan.notes {
        println!("  note: {note}");
    }
}

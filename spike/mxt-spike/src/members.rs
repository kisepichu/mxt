//! `members <room_id> [--filter TEXT]`: find a bridged user's Matrix ID for
//! question (6).

use anyhow::{Context as _, Result};
use matrix_sdk::{RoomMemberships, ruma::RoomId};

use crate::session::{self, DataDir};

pub async fn run(dir: &DataDir, room_id: &RoomId, filter: Option<&str>) -> Result<()> {
    let session = session::restore(dir).await?;
    let room = session.client.get_room(room_id).with_context(|| {
        format!("{room_id} is not in the store; run `rooms` first so the room is known")
    })?;
    let filter = filter.map(str::to_lowercase);
    // Fetches `/members` when the member list is not complete yet.
    let mut members = room.members(RoomMemberships::JOIN).await?;
    members.sort_by(|a, b| a.user_id().cmp(b.user_id()));
    let mut shown = 0;
    for member in &members {
        let display_name = member.display_name().unwrap_or("");
        if let Some(filter) = &filter
            && !member.user_id().as_str().to_lowercase().contains(filter)
            && !display_name.to_lowercase().contains(filter)
        {
            continue;
        }
        shown += 1;
        println!("{}\t{display_name}", member.user_id());
    }
    println!("{shown} of {} joined members", members.len());
    Ok(())
}

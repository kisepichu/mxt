//! `:join` (design, "Joining rooms and direct messages"): reading its
//! target, the pending invites, the wait for the adapter's answer, and the
//! question before a new direct message is created.

use super::{App, Confirm, StatusKind};
use crate::effect::{Effect, MatrixEffect};
use crate::fuzzy;
use crate::ids::{RoomId, UserId};
use crate::msg::JoinedHow;
use crate::room::Invite;

/// What a matrix.to link starts with.
const MATRIX_TO: &str = "https://matrix.to/#/";
/// The status while the adapter answers a `MatrixEffect::Join`.
const JOINING: &str = "Joining…";
/// The status while the adapter answers a `MatrixEffect::CreateDm`.
const CREATING: &str = "Creating a direct message…";

/// The answer the core waits for: the one to its latest `:join` or `y`.
/// The next key or paste ends the wait.
pub(super) enum JoinWait {
    /// The adapter's answer to the effect with this target.
    Answer { target: String },
    /// The answered room, which opens once the room list has it.
    Room { room: RoomId, how: JoinedHow },
}

impl App {
    /// `room.join`: an alias, a room ID, a user ID or a matrix.to link by
    /// its form goes to the adapter as typed, except that a room ID of a
    /// listed room opens at once; anything else is the name of a pending
    /// invite, whose room ID goes to the adapter.
    pub(super) fn join(&mut self, argument: &str, effects: &mut Vec<Effect>) {
        let target = if has_target_form(argument) {
            let room = RoomId::new(argument);
            if argument.starts_with('!') && self.rooms.get(&room).is_some() {
                return self.open_id(&room, effects);
            }
            argument.to_owned()
        } else {
            match invite_named(&self.invites, argument) {
                Ok(invite) => invite.room.to_string(),
                Err(error) => return self.set_status(StatusKind::Error, error),
            }
        };
        self.set_status(StatusKind::Info, JOINING);
        self.join_wait = Some(JoinWait::Answer {
            target: target.clone(),
        });
        effects.push(Effect::Matrix(MatrixEffect::Join { target }));
    }

    /// `y` to the question: the adapter looks once more and creates the
    /// direct message only when it still finds none.
    pub(super) fn create_dm(&mut self, user: UserId, effects: &mut Vec<Effect>) {
        self.set_status(StatusKind::Info, CREATING);
        self.join_wait = Some(JoinWait::Answer {
            target: user.to_string(),
        });
        effects.push(Effect::Matrix(MatrixEffect::CreateDm { user }));
    }

    /// `Msg::Joined`: the room opens now or once it is listed, when `target`
    /// is the one waited for.
    pub(super) fn joined(
        &mut self,
        target: &str,
        room: RoomId,
        how: JoinedHow,
        effects: &mut Vec<Effect>,
    ) {
        if self.waits_for(target) {
            self.join_wait = Some(JoinWait::Room { room, how });
            self.open_joined(effects);
        }
    }

    /// Opens the answered room once the room list has it, which ends the
    /// wait, with the status that says how the user got there.
    pub(super) fn open_joined(&mut self, effects: &mut Vec<Effect>) {
        let Some(JoinWait::Room { room, how }) = &self.join_wait else {
            return;
        };
        let Some(summary) = self.rooms.get(room) else {
            return;
        };
        let status = match how {
            JoinedHow::Already => None,
            JoinedHow::Joined => Some(format!("Joined {}", summary.name)),
            JoinedHow::Created => Some("Created a direct message".to_owned()),
        };
        let room = room.clone();
        self.join_wait = None;
        self.open_id(&room, effects);
        match status {
            Some(status) => self.set_status(StatusKind::Info, status),
            None => {
                if self
                    .status()
                    .is_some_and(|shown| shown.text == JOINING || shown.text == CREATING)
                {
                    self.status_shown = false;
                }
            }
        }
    }

    /// `Msg::NoDirectMessage`: the question while `target` is waited for;
    /// the status when nothing is, so a key typed before the answer never
    /// answers it; nothing while another `:join` is waited for.
    pub(super) fn no_direct_message(&mut self, target: &str, user: UserId) {
        if self.waits_for(target) {
            self.join_wait = None;
            self.confirm = Some(Confirm::CreateDm { user });
        } else if self.join_wait.is_none() {
            self.set_status(
                StatusKind::Info,
                "No direct message found; :join again to create one",
            );
        }
    }

    fn waits_for(&self, target: &str) -> bool {
        matches!(&self.join_wait, Some(JoinWait::Answer { target: waited }) if waited == target)
    }
}

/// An alias (`#` and text with a `:`), a room ID (`!` and text), a user ID
/// (`@` and text with a `:`) or a matrix.to link, without whitespace.
fn has_target_form(argument: &str) -> bool {
    if argument.contains(char::is_whitespace) {
        return false;
    }
    if let Some(rest) = argument.strip_prefix(['#', '@']) {
        return rest.contains(':');
    }
    if let Some(rest) = argument.strip_prefix('!') {
        return !rest.is_empty();
    }
    argument
        .strip_prefix(MATRIX_TO)
        .is_some_and(|rest| !rest.is_empty())
}

/// The pending invite whose name matches `query` best, as `:open` matches
/// room names; an error when none does, or when another invite has the
/// same name.
fn invite_named<'a>(invites: &'a [Invite], query: &str) -> Result<&'a Invite, String> {
    let names: Vec<&str> = invites.iter().map(|invite| invite.name.as_str()).collect();
    let Some(&best) = fuzzy::rank(query, &names).first() else {
        return Err(format!("No pending invite matches \"{query}\""));
    };
    let invite = &invites[best];
    if shares_its_name(invites, invite) {
        return Err(format!(
            "More than one pending invite is named \"{}\"; <Tab> completes their room IDs",
            invite.name
        ));
    }
    Ok(invite)
}

/// Whether another pending invite has `invite`'s name.
fn shares_its_name(invites: &[Invite], invite: &Invite) -> bool {
    invites
        .iter()
        .any(|other| other.name == invite.name && other.room != invite.room)
}

/// `:join`'s completion of `typed`: the pending invites ranked by name, as
/// `join <name>`, or `join <room ID>` for an invite whose name another
/// invite shares.
pub(super) fn join_candidates(typed: &str, invites: &[Invite]) -> Vec<String> {
    let names: Vec<&str> = invites.iter().map(|invite| invite.name.as_str()).collect();
    fuzzy::rank(typed, &names)
        .into_iter()
        .map(|index| {
            let invite = &invites[index];
            if shares_its_name(invites, invite) {
                format!("join {}", invite.room)
            } else {
                format!("join {}", invite.name)
            }
        })
        .collect()
}

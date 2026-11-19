//! The composers: per-room drafts and mentions, sending, replies and edits,
//! and the mention menu (design, "Terminal adapter and the composer").

use super::timeline::NOT_SENT;
use super::{App, Focus, StatusKind};
use crate::action::FocusMode;
use crate::effect::{Effect, MatrixEffect, Outgoing, OutgoingKind, UiEffect};
use crate::ids::{RoomId, SendId, UserId};
use crate::mention::{
    Member, MentionRef, mention_text, mentions_for_edit, rank_members, render_mentions,
};
use crate::msg::EditorMode;
use crate::timeline::{Draft, ItemRef, MessageContent, OwnEvent, TimelineEntry};

/// The most candidates the mention menu keeps, so a lone `@` in a room with
/// thousands of members does not copy all of them on every key.
const MENU_CANDIDATES: usize = 50;

/// A room's composer as the core knows it; the tui adapter owns the text.
/// Kept when switching rooms.
pub(super) struct Composer {
    draft: Draft,
    /// The mentions inserted so far; only those whose text is still present
    /// when sending count.
    mentions: Vec<MentionRef>,
    /// As the tui adapter last reported it, or as the core left it with
    /// `ClearComposer` and `LoadComposer`.
    empty: bool,
    /// The room's members, once loaded.
    members: Vec<Member>,
    /// Whether `LoadMembers` was requested for the room.
    members_requested: bool,
}

impl Default for Composer {
    fn default() -> Composer {
        Composer {
            draft: Draft::New,
            mentions: Vec::new(),
            empty: true,
            members: Vec::new(),
            members_requested: false,
        }
    }
}

/// The mention menu of the focused composer.
pub(super) struct MentionMenu {
    room: RoomId,
    /// The text after `@`.
    query: String,
    candidates: Vec<Member>,
    selected: usize,
}

/// A submitted draft, kept until its send is answered: queued (for an edit
/// of a sent message, sent) or failed (design, "Error handling").
pub(super) struct PendingSend {
    room: RoomId,
    /// The composer text as submitted, before the mentions were rendered.
    text: String,
    draft: Draft,
    mentions: Vec<MentionRef>,
}

impl App {
    /// The room's draft; `None` before its composer is used.
    pub fn draft(&self, room: &RoomId) -> Option<&Draft> {
        self.composers.get(room).map(|composer| &composer.draft)
    }

    /// The open mention menu: candidates and the selected index. Open while
    /// the composer has focus and the `@` token has candidates.
    pub fn mention_menu(&self) -> Option<(&[Member], usize)> {
        self.open_menu()
            .map(|menu| (menu.candidates.as_slice(), menu.selected))
    }

    fn open_menu(&self) -> Option<&MentionMenu> {
        self.mention_menu
            .as_ref()
            .filter(|menu| self.focus == Focus::Composer && !menu.candidates.is_empty())
    }

    /// `composer.focus`: the open room's composer gets the keys. The first
    /// time for a room, its members are requested for mention completion.
    /// A menu from an earlier focus is closed; the tui reports the token.
    /// With insert mode, keys resolved again after it are resolved in insert
    /// mode. With `Keep`, the core learns the mode with the next key.
    pub(super) fn focus_composer(&mut self, mode: FocusMode, effects: &mut Vec<Effect>) {
        let Some(room) = self.open_room.clone() else {
            return self.set_status(StatusKind::Error, "No room is open");
        };
        self.focus = Focus::Composer;
        self.mention_menu = None;
        if mode == FocusMode::Insert {
            self.editor = Some(EditorMode::Insert);
        }
        effects.push(Effect::Ui(UiEffect::FocusComposer {
            room: room.clone(),
            mode,
        }));
        let composer = self.composers.entry(room.clone()).or_default();
        if !std::mem::replace(&mut composer.members_requested, true) {
            effects.push(Effect::Matrix(MatrixEffect::LoadMembers(room)));
        }
    }

    /// `composer.send`, `composer.newline`, `composer.external_editor`: the
    /// tui adapter acts on the open room's editor.
    pub(super) fn composer_effect(
        &self,
        effect: fn(RoomId) -> UiEffect,
        effects: &mut Vec<Effect>,
    ) {
        if let Some(room) = &self.open_room {
            effects.push(Effect::Ui(effect(room.clone())));
        }
    }

    /// `Msg::Submit`: a blank body does nothing; otherwise the body is sent
    /// as the draft says, with the mentions still in it, and the composer
    /// starts over. The submitted draft is kept until the send is answered.
    pub(super) fn submit(&mut self, room: RoomId, body: String, effects: &mut Vec<Effect>) {
        if body.trim().is_empty() {
            return;
        }
        let composer = self.composers.entry(room.clone()).or_default();
        let kind = match &composer.draft {
            Draft::New => OutgoingKind::New,
            Draft::Reply { to } => OutgoingKind::Reply { to: to.clone() },
            Draft::Edit { target } => OutgoingKind::Edit {
                target: target.item().clone(),
            },
        };
        let rendered = render_mentions(&body, &composer.mentions);
        let id = SendId(self.next_send);
        self.next_send += 1;
        let pending = PendingSend {
            room: room.clone(),
            text: body,
            draft: std::mem::take(&mut composer.draft),
            mentions: std::mem::take(&mut composer.mentions),
        };
        self.pending_sends.insert(id, pending);
        effects.push(Effect::Matrix(MatrixEffect::Send {
            room: room.clone(),
            id,
            message: Outgoing {
                body: rendered,
                kind,
            },
        }));
        self.clear_composer(room, effects);
    }

    /// `Msg::SendQueued`: the local echo has the message now, or the edit of
    /// a sent message was sent.
    pub(super) fn send_queued(&mut self, id: SendId) {
        self.pending_sends.remove(&id);
    }

    /// `Msg::SendFailed`: the error goes to the status line, and the draft
    /// back into its room's composer if that room is open and its composer
    /// is as the send left it: empty, with a new draft. Otherwise the user
    /// has moved on and the draft is dropped.
    pub(super) fn send_failed(&mut self, id: SendId, error: String, effects: &mut Vec<Effect>) {
        self.set_status(StatusKind::Error, error);
        let Some(pending) = self.pending_sends.remove(&id) else {
            return;
        };
        if self.open_room.as_ref() != Some(&pending.room) {
            return;
        }
        let composer = self.composers.entry(pending.room.clone()).or_default();
        if !composer.empty || composer.draft != Draft::New {
            return;
        }
        composer.draft = pending.draft;
        composer.mentions = pending.mentions;
        // A submitted body is not blank.
        composer.empty = false;
        effects.push(Effect::Ui(UiEffect::LoadComposer {
            room: pending.room,
            text: pending.text,
        }));
    }

    /// `composer.cancel`: a reply draft becomes new and keeps its text; an
    /// edit draft becomes new and the composer is cleared.
    pub(super) fn cancel_draft(&mut self, effects: &mut Vec<Effect>) {
        let Some(room) = self.open_room.clone() else {
            return;
        };
        let Some(composer) = self.composers.get_mut(&room) else {
            return;
        };
        match std::mem::take(&mut composer.draft) {
            Draft::New | Draft::Reply { .. } => {}
            Draft::Edit { .. } => self.clear_composer(room, effects),
        }
    }

    /// Empties the room's composer: a new draft without mentions or menu.
    fn clear_composer(&mut self, room: RoomId, effects: &mut Vec<Effect>) {
        let composer = self.composers.entry(room.clone()).or_default();
        composer.draft = Draft::New;
        composer.mentions.clear();
        composer.empty = true;
        self.mention_menu.take_if(|menu| menu.room == room);
        effects.push(Effect::Ui(UiEffect::ClearComposer(room)));
    }

    /// `Msg::ComposerEdited`.
    pub(super) fn composer_edited(&mut self, room: RoomId, empty: bool) {
        self.composers.entry(room).or_default().empty = empty;
    }

    /// `message.reply`: a reply to the remote item under the cursor, in the
    /// composer. A local echo is refused: the SDK replaces it with its remote
    /// event, which a draft would not follow. An edit draft is refused too:
    /// the composer holds the edit's text.
    pub(super) fn reply(&mut self, effects: &mut Vec<Effect>) {
        let (Some(room), Some(item)) = (&self.open_room, self.timeline.cursor_item()) else {
            return;
        };
        let ItemRef::Remote(to) = &item.id else {
            return self.set_status(StatusKind::Error, NOT_SENT);
        };
        if self
            .composers
            .get(room)
            .is_some_and(|composer| matches!(composer.draft, Draft::Edit { .. }))
        {
            return self.set_status(StatusKind::Error, "Cancel the edit first");
        }
        let to = to.clone();
        self.composers.entry(room.clone()).or_default().draft = Draft::Reply { to };
        self.focus_composer(FocusMode::Insert, effects);
    }

    /// `message.edit`: the user's own text message under the cursor, its
    /// plain body loaded into the empty composer with its mentions rebuilt.
    pub(super) fn edit_message(&mut self, effects: &mut Vec<Effect>) {
        let (Some(room), Some(item)) = (&self.open_room, self.timeline.cursor_item()) else {
            return;
        };
        let Some(target) = OwnEvent::new(item, &self.me) else {
            return self.set_status(StatusKind::Error, "You can only edit your own messages");
        };
        // Other messages cannot become text without losing what they are.
        let MessageContent::Text { body, rich } = &item.content else {
            return self.set_status(StatusKind::Error, "Only text messages can be edited");
        };
        if self
            .composers
            .get(room)
            .is_some_and(|composer| !composer.empty)
        {
            return self.set_status(StatusKind::Error, "Clear the composer first");
        }
        // The plain body, not `rich.to_plain()`: for messages mxt sent it is
        // the Markdown that was typed, with its links and code intact.
        let text = body.clone();
        let mentions = mentions_for_edit(rich, body);
        let room = room.clone();
        let composer = self.composers.entry(room.clone()).or_default();
        composer.draft = Draft::Edit { target };
        composer.mentions = mentions;
        composer.empty = text.is_empty();
        self.focus_composer(FocusMode::Insert, effects);
        effects.push(Effect::Ui(UiEffect::LoadComposer { room, text }));
    }

    /// `Msg::MentionQuery` for the focused composer: opens or updates the
    /// menu, or closes it without a token.
    pub(super) fn mention_query(&mut self, room: RoomId, query: Option<String>) {
        if self.focus != Focus::Composer || self.open_room.as_ref() != Some(&room) {
            return;
        }
        self.mention_menu = query.map(|query| MentionMenu {
            candidates: self.candidates(&room, &query),
            room,
            query,
            selected: 0,
        });
    }

    /// `Msg::MembersLoaded`: replaces the room's members and ranks an open
    /// menu of the room again.
    pub(super) fn members_loaded(&mut self, room: RoomId, members: Vec<Member>) {
        let composer = self.composers.entry(room.clone()).or_default();
        composer.members = members;
        composer.members_requested = true;
        if let Some(menu) = &self.mention_menu
            && menu.room == room
        {
            let candidates = self.candidates(&room, &menu.query);
            if let Some(menu) = &mut self.mention_menu {
                menu.candidates = candidates;
                menu.selected = 0;
            }
        }
    }

    /// The room's members ranked against `query`, the senders of the loaded
    /// messages first, at most [`MENU_CANDIDATES`]. The user is not ranked
    /// as a sender: their own messages say nothing about whom they want to
    /// mention.
    fn candidates(&self, room: &RoomId, query: &str) -> Vec<Member> {
        let members = self
            .composers
            .get(room)
            .map_or(&[][..], |composer| &composer.members);
        let mut senders: Vec<UserId> = Vec::new();
        for entry in self.timeline.entries().iter().rev() {
            if let TimelineEntry::Item(item) = entry
                && item.sender != self.me
                && !senders.contains(&item.sender)
            {
                senders.push(item.sender.clone());
            }
        }
        rank_members(query, members, &senders)
            .into_iter()
            .take(MENU_CANDIDATES)
            .cloned()
            .collect()
    }

    /// `completion.next` / `completion.prev`, wrapping around.
    pub(super) fn move_completion(&mut self, forward: bool) {
        let Some(menu) = self.open_menu() else {
            return;
        };
        // An open menu has candidates.
        let last = menu.candidates.len() - 1;
        let selected = match (forward, menu.selected) {
            (true, selected) if selected >= last => 0,
            (true, selected) => selected + 1,
            (false, 0) => last,
            (false, selected) => selected - 1,
        };
        if let Some(menu) = &mut self.mention_menu {
            menu.selected = selected;
        }
    }

    /// `completion.accept`: records the selected member's mention and has
    /// the tui replace the token with its text.
    pub(super) fn accept_completion(&mut self, effects: &mut Vec<Effect>) {
        let Some(menu) = self.open_menu() else {
            return;
        };
        let Some(member) = menu.candidates.get(menu.selected).cloned() else {
            return;
        };
        let room = menu.room.clone();
        let composer = self.composers.entry(room.clone()).or_default();
        let mention = MentionRef {
            text: mention_text(&member, &composer.members),
            user: member.user,
        };
        let text = mention.text.clone();
        if !composer.mentions.contains(&mention) {
            composer.mentions.push(mention);
        }
        self.mention_menu = None;
        effects.push(Effect::Ui(UiEffect::ReplaceMentionToken { room, text }));
    }

    /// `completion.cancel`.
    pub(super) fn cancel_completion(&mut self) {
        self.mention_menu = None;
    }
}

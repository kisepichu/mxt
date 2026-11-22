//! Threads (design, "Threads"): a thread shown in place of the open room's
//! main timeline, which is kept aside meanwhile, `thread.open` and
//! `thread.close`.

use super::jumps::AfterGone;
use super::timeline::{NOT_SENT, Timeline};
use super::{App, Focus, StatusKind, View};
use crate::effect::{Effect, MatrixEffect};
use crate::ids::{EventId, TimelineId};
use crate::timeline::{ItemRef, MessageContent, TimelineItem};

/// The thread shown in place of the open room's main timeline.
pub(super) struct OpenThread {
    /// The root's event ID.
    pub(super) root: EventId,
    /// The main timeline, kept aside with its cursor, pagination, receipt
    /// and `read_up_to`. Its diffs still apply, so it is current when the
    /// thread closes.
    pub(super) main: Timeline,
}

/// The shown thread, for the pane's title row.
pub struct ThreadView<'a> {
    pub root: &'a EventId,
    /// The root's item, from the thread's entries, else from the main
    /// timeline's; `None` while neither holds it.
    pub item: Option<&'a TimelineItem>,
}

impl App {
    /// The thread shown in place of the main timeline.
    pub fn thread(&self) -> Option<ThreadView<'_>> {
        let open = self.thread.as_ref()?;
        let item = self
            .timeline
            .event_item(&open.root)
            .or_else(|| open.main.event_item(&open.root));
        Some(ThreadView {
            root: &open.root,
            item,
        })
    }

    /// The open room's main timeline, shown or kept aside for a thread.
    pub(super) fn main_timeline_mut(&mut self) -> &mut Timeline {
        match &mut self.thread {
            Some(open) => &mut open.main,
            None => &mut self.timeline,
        }
    }

    /// `thread.open`: shows the thread whose root is the item under the
    /// cursor, with or without replies, and records the jump. A local echo
    /// is refused (a root needs its event ID), and so is a deleted message
    /// without replies. In a thread, whose items all belong to it, it only
    /// keeps the focus on the timeline.
    pub(super) fn open_thread(&mut self, effects: &mut Vec<Effect>) {
        if self.thread.is_some() {
            self.focus = Focus::Timeline;
            return;
        }
        let Some(item) = self.timeline.cursor_item() else {
            return;
        };
        let root = match &item.id {
            ItemRef::Local(_) => return self.set_status(StatusKind::Error, NOT_SENT),
            ItemRef::Remote(_)
                if item.content == MessageContent::Redacted && item.thread_summary.is_none() =>
            {
                return self.set_status(StatusKind::Error, "The message is deleted");
            }
            ItemRef::Remote(event) => event.clone(),
        };
        self.record_jump();
        self.enter_thread(root, effects);
    }

    /// `thread.close`: records the position in the thread as a jump and
    /// shows the main timeline again with the cursor on the root. When the
    /// main timeline does not hold the root, older messages load until it
    /// does; when its start loads without it, the cursor follows the newest
    /// message once the grace period passes.
    pub(super) fn close_thread(&mut self, effects: &mut Vec<Effect>) {
        if self.thread.is_none() {
            return;
        }
        self.record_jump();
        if let Some(root) = self.leave_thread(effects) {
            self.place_or_wait(root, AfterGone::Follow, effects);
        }
    }

    /// Shows `timeline` of the open room without recording a jump, as going
    /// to a jump list entry does: opens its thread in place of the main
    /// timeline or of another thread, keeps it when it is the one shown, or
    /// closes an open thread for the main timeline.
    pub(super) fn show_timeline(&mut self, timeline: &TimelineId, effects: &mut Vec<Effect>) {
        let shown = self.thread.as_ref().map(|open| &open.root);
        if timeline.thread.as_ref() == shown {
            return;
        }
        self.leave_thread(effects);
        if let Some(root) = &timeline.thread {
            self.enter_thread(root.clone(), effects);
        }
    }

    /// Shows the open room's thread rooted at `root` with a new, empty
    /// timeline in place of the main timeline, which goes aside, and
    /// subscribes it.
    fn enter_thread(&mut self, root: EventId, effects: &mut Vec<Effect>) {
        let Some(room) = self.open_room.clone() else {
            return;
        };
        effects.push(Effect::Matrix(MatrixEffect::SubscribeThread {
            room,
            root: root.clone(),
        }));
        let main = std::mem::take(&mut self.timeline);
        self.thread = Some(OpenThread { root, main });
        self.replace_pane(effects);
    }

    /// Puts the main timeline back in place of the open thread and
    /// unsubscribes the thread. The thread's root; `None` without a thread.
    pub(super) fn leave_thread(&mut self, effects: &mut Vec<Effect>) -> Option<EventId> {
        let open = self.thread.take()?;
        if let Some(room) = &self.open_room {
            effects.push(Effect::Matrix(MatrixEffect::UnsubscribeThread {
                room: room.clone(),
                root: open.root.clone(),
            }));
        }
        self.timeline = open.main;
        self.replace_pane(effects);
        Some(open.root)
    }

    /// Closes what belongs to the pane when it shows another timeline of
    /// the room, as opening another room does: its viewport, which must not
    /// count for the other timeline's entries before that is drawn, the
    /// wait for a jump's message, the previews it wants (those in flight are
    /// still answered), a message view (a help view stays), the mention
    /// menu and the composer's focus; the timeline gets the focus.
    fn replace_pane(&mut self, effects: &mut Vec<Effect>) {
        self.timeline_viewport = None;
        self.jump_wait = None;
        self.drop_wanted_previews();
        if matches!(self.view, Some(View::Message(_))) {
            self.close_view(effects);
        }
        self.mention_menu = None;
        self.focus = Focus::Timeline;
    }
}

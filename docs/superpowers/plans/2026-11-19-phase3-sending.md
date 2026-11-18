# Phase 3 (sending) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Messages can be written day to day: per-room edtui composers,
Markdown with mentions that notify people on the remote network, replies,
edits, deletion, reactions with an emoji picker, and the external editor.

**Architecture:** The core owns each room's draft (`Draft`, the inserted
`MentionRef`s, whether the composer is empty), the mention menu, the emoji
picker, the confirmation prompt and the rules for sending, editing and
deleting; it renders mentions into the plain body, the Markdown for the HTML
body and the mentioned user IDs. The tui adapter owns the edtui editor
states and reports edits. The matrix adapter sends through the open room's
`Timeline`.

**Tech Stack:** Rust 2024, edtui 0.11, pulldown-cmark 0.13 (offsets only),
nucleo-matcher, emojis 0.9, matrix-sdk-ui 0.19 `Timeline`, ruma
`FormattedBody::markdown`.

## Global Constraints

- The design is `docs/superpowers/specs/2026-11-18-mxt-design.md`. It wins over
  this plan when they disagree.
- Everything in the repository is in English. Tests use made-up identifiers.
- `mxt-core` depends only on `std` and small pure crates (`pulldown-cmark`,
  `emojis`, `nucleo-matcher`, `unicode-segmentation` are allowed); no I/O.
- Message bodies are logged only at `trace`.
- Writes to a real account happen only in the test room, by the
  orchestrator. Workers never send anything.
- Dependencies are declared once in the root `[workspace.dependencies]`; all
  cargo commands in `nix develop`; matrix-sdk builds under the shared lock.

---

### Task 1: `render_mentions` and mention candidates

**Files:** Create `crates/mxt-core/src/mention.rs`; modify `lib.rs`,
root `Cargo.toml` (`pulldown-cmark = { version = "0.13", default-features = false }`).

**Interfaces:**

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionRef { pub user: UserId, pub text: String }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedBody {
    /// The editor text unchanged.
    pub plain: String,
    /// Markdown for the HTML body: remaining mention texts linked.
    pub markdown: String,
    /// The users whose mention text is still present, in first-occurrence order, without duplicates.
    pub user_ids: Vec<UserId>,
}
/// Design, "Matrix adapter" (Sending).
pub fn render_mentions(text: &str, mentions: &[MentionRef]) -> RenderedBody;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member { pub user: UserId, pub display_name: Option<String> }

/// Ranks members against `query` (the text after `@`) with nucleo on the
/// display name and the user ID. Members who sent one of the loaded messages
/// come first, newest sender first (`recent_senders`, newest first); the rest
/// follow by match score. An empty query lists the recent senders first, then
/// everyone else by display name.
pub fn rank_members<'a>(query: &str, members: &'a [Member], recent_senders: &[UserId]) -> Vec<&'a Member>;

/// The text inserted for `member`: `@<display name>`, or `@<user ID>` when
/// another member has the same display name or the member has none.
pub fn mention_text(member: &Member, members: &[Member]) -> String;

/// Rebuilds the mentions of a message being edited: the mention spans of its
/// `RichText` whose text appears in `body`.
pub fn mentions_for_edit(content: &RichText, body: &str) -> Vec<MentionRef>;
```

**Rules for `render_mentions`:** find the inline code and code block ranges
of `text` with `pulldown_cmark::Parser::new_ext(text, Options::empty()).into_offset_iter()`
(`Event::Code`, `Tag::CodeBlock` ranges). For each mention (longest text
first, so `@Al` inside `@Alice` does not match), every occurrence of its text
outside those ranges becomes `[<escaped text>](https://matrix.to/#/<user ID>)`,
where Markdown special characters in the text (`\ ` ` * _ [ ] ( ) # + - . ! < > | ~`)
are backslash-escaped. The occurrence must start at the start of the text or
after whitespace and end at the end of the text, before whitespace or before
punctuation, so `@Bob` does not match inside `@Bobby`.

**Tests:** the plain body is unchanged; a deleted mention text is dropped
from markdown and user IDs; a mention inside inline code and inside a fenced
code block is not linked; Markdown in a display name (`@A*b*_c`) is escaped;
two mentions of the same user give one user ID; `@Al` and `@Alice` both
inserted link correctly; `@Bob` is not linked inside `@Bobby`; a text with no
mentions gives `markdown == plain` and no user IDs. `rank_members`: recent
senders first, newest first; fuzzy on display name and user ID; empty query
order. `mention_text`: a duplicate display name gives the user ID; a missing
one gives the user ID. `mentions_for_edit`: only spans still present.

- [ ] Steps: failing tests, see them fail, implement, pass, fmt and clippy, commit
  ("Render mentions into plain text, Markdown and user IDs, and rank mention candidates, in the core").

---

### Task 2: Composer, drafts, mention menu, confirmation and the emoji picker in the core

**Files:** Create `crates/mxt-core/src/app/composer.rs`,
`crates/mxt-core/src/app/picker.rs`, `crates/mxt-core/src/emoji.rs`; modify
`app.rs`, `msg.rs`, `effect.rs`, `timeline.rs`, `lib.rs`, root `Cargo.toml`
(`emojis = "0.9"`), tests.

**Interfaces (additions):**

```rust
// timeline.rs (design, "Functional core")
pub enum Draft { New, Reply { to: ItemRef }, Edit { target: OwnEvent } }

// App::new gains the user's ID (needed for OwnEvent and mention ranking):
pub fn new(keymap: Keymap, settings: Settings, me: UserId) -> App;

// msg.rs
Msg::Submit { room: RoomId, body: String },
Msg::ComposerEdited { room: RoomId, empty: bool },
Msg::MentionQuery { room: RoomId, query: Option<String> },
Msg::MembersLoaded { room: RoomId, members: Vec<Member> },
Msg::Sent { room: RoomId },            // optional; failures arrive as EffectFailed or on the local echo

// effect.rs
pub struct Outgoing { pub body: RenderedBody, pub kind: OutgoingKind }
pub enum OutgoingKind { New, Reply { to: EventId }, Edit { target: ItemRef } }
MatrixEffect::Send { room: RoomId, message: Outgoing },
MatrixEffect::LoadMembers(RoomId),
MatrixEffect::Redact { room: RoomId, item: ItemRef },
MatrixEffect::ToggleReaction { room: RoomId, item: ItemRef, key: String },
MatrixEffect::Resend { room: RoomId, txn: TxnId },
UiEffect::FocusComposer { room: RoomId, mode: FocusMode },
UiEffect::RequestSubmit(RoomId),       // the tui reads the editor and answers Msg::Submit
UiEffect::InsertNewline(RoomId),
UiEffect::LoadComposer { room: RoomId, text: String },
UiEffect::ClearComposer(RoomId),
UiEffect::ReplaceMentionToken { room: RoomId, text: String },
UiEffect::ExternalEditor(RoomId),

// read access
impl App {
    pub fn draft(&self, room: &RoomId) -> Option<&Draft>;
    /// The open mention menu: candidates and the selected index.
    pub fn mention_menu(&self) -> Option<(&[Member], usize)>;
    pub fn picker(&self) -> Option<&Picker>;
    /// The confirmation question while in CONFIRM mode.
    pub fn confirmation(&self) -> Option<&str>;
}
pub struct Picker { pub kind: PickerKind, pub query: String, pub items: Vec<PickerItem>, pub selected: usize }
pub enum PickerKind { Emoji { room: RoomId, item: ItemRef } }   // phase 4 adds Rooms
pub struct PickerItem { pub label: String, pub key: String }
```

**Behavior** (design, "Terminal adapter and the composer", "Mention
completion", "Error handling"):
- Composers are per room and kept when switching rooms: the `Draft`, the
  `MentionRef`s, `empty` (from `Msg::ComposerEdited`), whether members were
  loaded.
- `composer.focus` (`i`, `a`): needs an open room; focus `Composer` and
  `UiEffect::FocusComposer { room, mode }`; the first time for the room also
  `MatrixEffect::LoadMembers(room)`. The input context then follows the
  editor mode the tui reports with every key (phase 1 rules), and
  `ComposerCompletion` while the mention menu is open in insert mode.
- `composer.send` → `UiEffect::RequestSubmit(room)`. `Msg::Submit`: a blank
  body does nothing; otherwise `MatrixEffect::Send` with
  `render_mentions(body, mentions)` and the kind from the draft (a reply to a
  local echo cannot be sent: status `Wait until the message is sent`), then
  `UiEffect::ClearComposer(room)`, the draft returns to `New` and the
  mentions are cleared.
- `composer.newline` → `UiEffect::InsertNewline(room)`.
- `composer.cancel`: a reply draft becomes `New` and keeps its text; an edit
  draft becomes `New` and `UiEffect::ClearComposer`.
- `composer.external_editor` → `UiEffect::ExternalEditor(room)`.
- `message.reply`: the item under the timeline cursor; `Draft::Reply { to }`,
  then focus the composer (insert).
- `message.edit`: own items only (`OwnEvent::new` with `me`; otherwise status
  `You can only edit your own messages`); requires an empty composer
  (otherwise status `Clear the composer first`); `Draft::Edit`,
  `UiEffect::LoadComposer { text: plain body }`, mentions from
  `mentions_for_edit`, focus the composer.
- `message.delete`: own items only; CONFIRM mode with `Delete this message? (y/n)`;
  `y` → `MatrixEffect::Redact`; any other key cancels. CONFIRM takes keys
  before the keymap (`mode_label` `CONFIRM`).
- `message.react`: opens the emoji picker for the item under the cursor
  (`PICKER` mode, `InputContext::Picker`, text sink to the query). Items:
  every emoji from `emojis::iter()` with its first shortcode as the label
  (`:+1: 👍`), ranked with `fuzzy::rank` on the shortcodes and name; an empty
  query lists the emojis the user already reacted with on that item first,
  then the most common reactions (`👍 ❤️ 😂 😮 😢 🎉 👀 🙏`), then the rest
  in `emojis` order. `picker.next`/`picker.prev` move the selection;
  `picker.accept` → `MatrixEffect::ToggleReaction { key: emoji }` and close;
  `picker.cancel` closes.
- `message.resend`: a failed local echo under the cursor →
  `MatrixEffect::Resend { txn }`; otherwise status `Nothing to resend`.
- Mention menu: `Msg::MentionQuery { query: Some(q) }` for the room whose
  composer has focus opens or updates the menu with `rank_members(q, members,
  recent senders of the loaded timeline)`; `None` closes it.
  `completion.next`/`prev` move the selection; `completion.accept` records
  `MentionRef { user, text: mention_text(..) }` and returns
  `UiEffect::ReplaceMentionToken { text }` (the tui adds the space) and
  closes the menu; `completion.cancel` closes it.
- `Msg::MembersLoaded` stores the members (replacing earlier ones).

**Tests:** drafts kept per room across room switches; `composer.cancel` for
reply and edit drafts; the empty-composer rule and own-message rule for
`message.edit`, with mentions rebuilt; `Msg::Submit` blank, new, reply,
edit, and reply to a local echo; members loaded once per room; mention menu
open, rank, accept (recorded mention, effect), cancel, close on `None`;
`message.delete` confirm and cancel; emoji picker query, ordering of an
empty query, accept and cancel; `message.resend`.

- [ ] Steps: failing tests, see them fail, implement, pass, fmt and clippy, commit
  ("Add composers, drafts, mention completion, confirmation and the emoji picker to the core").

---

### Task 3: Sending, reactions, redaction and members in the matrix adapter

**Files:** modify `crates/mxt-matrix/src/adapter.rs`, `crates/mxt-matrix/src/timeline.rs`.

**Behavior:**
- `Send`: `FormattedBody::markdown(&message.body.markdown)`: `Some(html)` →
  `RoomMessageEventContent::text_html(plain, html)`, `None` →
  `text_plain(plain)`; add `Mentions::with_user_ids(user_ids)` when not empty.
  `New` → `timeline.send(content.into())`; `Reply { to }` →
  `timeline.send_reply(content, to)`; `Edit { target }` →
  `timeline.edit(&item_id, EditedContent::RoomMessage(..))` (keep the
  mentions). The local echo shows the send state; a failure to queue becomes
  `Msg::EffectFailed`.
- `ToggleReaction` → `timeline.toggle_reaction(&item_id, &key)`.
- `Redact` → `timeline.redact(&item_id, None)`.
- `Resend` → the send queue's retry for the local echo (look up the 0.19 API
  in the sources).
- `LoadMembers` → `room.members(RoomMemberships::JOIN)`, converted to
  `Member`s, then `Msg::MembersLoaded`. Large rooms: send what loaded.
- Tests: the content building (`text_html` vs `text_plain`, mentions) as a
  pure function from `Outgoing`.

- [ ] Steps: tests, implement, pass (under the lock), fmt and clippy, commit
  ("Send messages, replies, edits, reactions and redactions through the open room's timeline").

---

### Task 4: Composers, menus, the picker and the external editor in the terminal adapter

**Files:** Create `crates/mxt-tui/src/composer.rs`,
`crates/mxt-tui/src/overlay.rs`; modify `terminal.rs`, `view.rs`, `input.rs`.

**Behavior:**
- One edtui `EditorState` per room (vim mode, the shared register), created
  on first focus; the composer sits under the timeline, one line high when
  empty and growing with its text up to a third of the pane.
- While the composer has focus, every key goes to the core as
  `Msg::Key { editor: Some(mode) }`; `ReplayKeys` feed the editor. After each
  replayed key: report `Msg::ComposerEdited { empty }` when emptiness
  changed, and in insert mode the `@` token at the cursor as
  `Msg::MentionQuery` when it changed (design: an `@` at the start of the
  text or after whitespace, followed by non-whitespace).
- Effects: `FocusComposer` (insert mode unless `Keep`), `RequestSubmit`
  (answer `Msg::Submit` with the text), `InsertNewline`, `LoadComposer`,
  `ClearComposer`, `ReplaceMentionToken` (replace the token with the text
  and a space), `ExternalEditor` (edtui's `system-editor`: leave the
  alternate screen and raw mode, run the editor, restore, redraw).
- The terminal cursor is at the composer's `cursor_screen_position()` while
  it has focus (phase 0 finding, for input method placement).
- Overlays: the mention menu above the composer; the emoji picker as a
  centered list with the query line; the CONFIRM question in the status line.
- The status line shows the editor mode while the composer has focus.
- Verify by running (throwaway example as in phase 2): Japanese input via
  `tmux send-keys`, mention menu, `<C-j>` newlines, the picker, CONFIRM, and
  the external editor with `EDITOR=true`.

- [ ] Steps: unit tests for the `@` token rule and emptiness tracking,
  implement, verify by running, fmt and clippy, commit
  ("Add the composers, the mention menu, the emoji picker and the external editor to the terminal adapter").

---

### Task 5: Wiring and the check in the test room

- `main` passes the user ID to `App::new`; effects and messages route as
  before.
- The orchestrator checks in the test room only: sending plain and Markdown
  text, a reply, an edit, a deletion, a reaction toggle, a mention of their
  own user, the external editor. The bridged mention check (phase 0 question
  6) is the owner's.

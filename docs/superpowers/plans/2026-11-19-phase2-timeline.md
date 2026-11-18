# Phase 2 (timeline) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rooms can be read day to day: the open room's timeline with HTML
bodies, replies, edits and reactions, date dividers and the read marker,
back pagination, read receipts, and a full-screen message view for messages
taller than the pane.

**Architecture:** The core owns the open room's timeline entries, the
timeline cursor, pagination requests and the receipt rule, and receives SDK
timeline changes as diffs. The matrix adapter subscribes one `Timeline`
(matrix-sdk-ui) for the open room, converts its items into core entries, and
renders HTML bodies into core `RichText` with `ruma::html`. The tui adapter
draws the timeline, reports viewports, and shows the message view in an
edtui scratch editor.

**Tech Stack:** Rust 2024, matrix-sdk-ui 0.19 `Timeline`, `ruma::html`
(feature `html-matrix`), ratatui, edtui 0.11, jiff.

## Global Constraints

- The design is `docs/superpowers/specs/2026-11-18-mxt-design.md`. It wins over
  this plan when they disagree.
- Everything in the repository is in English. Tests use made-up identifiers.
- `mxt-core` depends only on `std` and small pure crates; no SDK, `ruma`,
  async runtime, terminal library, `serde` or `toml`. No I/O, no clock.
- Message bodies are logged only at `trace`.
- Dependencies are declared once in the root `[workspace.dependencies]`.
- All cargo commands run inside `nix develop`; anything that compiles
  matrix-sdk runs under the shared build lock.
- Checks before every commit: fmt, clippy with `-D warnings`, tests.

## File Structure

| Path | Responsibility |
|---|---|
| `crates/mxt-core/src/rich.rs` | `RichText` and its plain-text form |
| `crates/mxt-core/src/timeline.rs` | timeline entries, `ItemRef`, `OwnEvent`, `MessageContent`, diffs |
| `crates/mxt-core/src/app/timeline.rs` | timeline state, cursor, pagination, receipts, reply jumps, yank, view |
| `crates/mxt-matrix/src/html.rs` | `formatted_body` to `RichText` |
| `crates/mxt-matrix/src/timeline.rs` | `Timeline` subscription and item conversion |
| `crates/mxt-tui/src/timeline_view.rs` | timeline pane drawing |
| `crates/mxt-tui/src/message_view.rs` | the full-screen message view (edtui) |
| `crates/mxt-tui/src/clipboard.rs` | OSC 52 and the shared register |

---

### Task 1: Core timeline and rich text types

**Files:**
- Create: `crates/mxt-core/src/rich.rs`, `crates/mxt-core/src/timeline.rs`
- Modify: `crates/mxt-core/src/lib.rs`

**Interfaces:**

```rust
// rich.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Style { pub bold: bool, pub italic: bool, pub strike: bool, pub code: bool, pub spoiler: bool }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MentionTarget { User(UserId), Room(String) }   // room ID or alias as written

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text { text: String, style: Style },
    Link { text: String, url: String, style: Style },
    Mention { text: String, target: MentionTarget },
    /// A custom emoticon, shown by its alt text.
    Emoticon { alt: String },
    LineBreak,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Code { language: Option<String>, text: String },
    Quote(Vec<Block>),
    List { ordered: bool, start: u64, items: Vec<Vec<Block>> },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RichText { pub blocks: Vec<Block> }
impl RichText {
    /// One paragraph per line of `text`, line breaks kept.
    pub fn plain(text: &str) -> RichText;
    /// The text as a reader sees it: inline text, link text, mention text,
    /// emoticon alt text; blocks separated by newlines; list items prefixed
    /// with `- ` or `<n>. `; quote lines prefixed with `> `; code blocks as
    /// their text.
    pub fn to_plain(&self) -> String;
    /// Mention spans, in order (used in phase 3 to rebuild mentions for edits).
    pub fn mentions(&self) -> Vec<(&str, &MentionTarget)>;
}

// timeline.rs
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemRef { Local(TxnId), Remote(EventId) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendState { Sending, Sent, Failed { error: String } }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyPreview {
    pub event: EventId,
    /// `None` while the replied-to event is not available.
    pub sender_name: Option<String>,
    pub body: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReactionGroup { pub key: String, pub count: u32, pub mine: bool }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageContent {
    Text(RichText), Notice(RichText), Emote(RichText),
    Image { name: String, size: Option<u64>, dimensions: Option<(u32, u32)> },
    /// Also video and audio.
    File { name: String, size: Option<u64>, mime: Option<String> },
    Sticker { body: String },
    Redacted,
    Undecryptable,
    Unsupported { kind: String },
}
impl MessageContent {
    /// Searchable and yankable text: the rich text's plain form, the file or
    /// image name, the sticker body; `(deleted)`, `(unable to decrypt)`,
    /// `(unsupported: <kind>)` for the others.
    pub fn plain_text(&self) -> String;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineItem {
    pub id: ItemRef,
    pub sender: UserId,
    pub sender_name: String,
    pub timestamp: Timestamp,
    pub content: MessageContent,
    pub reply: Option<ReplyPreview>,
    pub reactions: Vec<ReactionGroup>,
    pub edited: bool,
    /// `Some` for local echoes.
    pub send_state: Option<SendState>,
}

/// One entry of the timeline, in the SDK's order (oldest first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineEntry {
    Item(TimelineItem),
    /// Start of a day; the timestamp of the first item of that day.
    DateDivider(Timestamp),
    ReadMarker,
    /// Anything the first release does not show (state events, membership
    /// changes). Kept so indices match the SDK's.
    Hidden,
}

/// Mirrors eyeball-im's `VectorDiff`, so the adapter can forward SDK diffs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineDiff {
    Append(Vec<TimelineEntry>), Clear, PushFront(TimelineEntry), PushBack(TimelineEntry),
    PopFront, PopBack, Insert(usize, TimelineEntry), Set(usize, TimelineEntry),
    Remove(usize), Truncate(usize), Reset(Vec<TimelineEntry>),
}
/// Applies one diff. Out-of-range indices are ignored (and are a bug in the
/// sender), never a panic.
pub fn apply_diff(entries: &mut Vec<TimelineEntry>, diff: TimelineDiff);

/// An item sent by the current user (design, "Functional core").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnEvent(ItemRef);
impl OwnEvent {
    pub fn new(item: &TimelineItem, me: &UserId) -> Option<OwnEvent>;
    pub fn item(&self) -> &ItemRef;
}
```

**Tests:** `RichText::plain` and `to_plain` round trip for multi-line text;
`to_plain` of a paragraph with bold text, a link and a mention; of a quote
containing a list (`> - a`, `> - b`); of an ordered list starting at 3; of a
code block. `mentions` returns user and room mentions in order.
`MessageContent::plain_text` for each variant. `apply_diff` for every
variant, including `Insert` at the end, `Truncate` beyond the length, and an
out-of-range `Set` that changes nothing. `OwnEvent::new` returns `Some` only
for the user's own items, for local and remote ids.

- [ ] Steps: failing tests, see them fail (`nix develop -c cargo test -p mxt-core`), implement, pass, fmt and clippy, commit
  ("Add the core timeline and rich text types, so timeline items reach the core as plain data").

---

### Task 2: Timeline state and actions in the core

**Files:**
- Create: `crates/mxt-core/src/app/timeline.rs`
- Modify: `crates/mxt-core/src/app.rs`, `crates/mxt-core/src/msg.rs`,
  `crates/mxt-core/src/effect.rs`, `crates/mxt-core/src/app/tests.rs` (or a
  new `app/timeline_tests.rs`)

**Interfaces** (additions):

```rust
// msg.rs
Msg::Timeline { room: RoomId, diffs: Vec<TimelineDiff> },
Msg::PaginationDone { room: RoomId, reached_start: bool },
Msg::PaginationFailed { room: RoomId, error: String },

// effect.rs
MatrixEffect::PaginateBackwards(RoomId),
/// A receipt of the configured kind and the fully read marker, both on `event`.
MatrixEffect::SendReceipt { room: RoomId, event: EventId },
UiEffect::OpenView(ViewContent),
UiEffect::CloseView,
/// Copy to the clipboard (OSC 52) and the shared register.
UiEffect::Yank(String),

pub enum ViewContent {
    /// The message's text for the scratch editor.
    Text(String),
}

// app.rs (read access for rendering)
impl App {
    /// The open room's entries, oldest first (empty when no room is open).
    pub fn timeline(&self) -> &[TimelineEntry];
    /// Index of the timeline cursor in `timeline()`, always on an `Item`.
    pub fn timeline_cursor(&self) -> Option<usize>;
    /// Whether the timeline follows the newest item.
    pub fn timeline_follows(&self) -> bool;
    pub fn view_open(&self) -> bool;
}
```

**Behavior:**
- Opening a room clears the timeline state (entries, cursor, pagination,
  receipt) before `SubscribeTimeline`. `Msg::Timeline` and the pagination
  messages for any other room are ignored.
- Cursor: on `Item` entries only (dividers, the read marker and hidden
  entries are skipped). It starts following the newest item. While it
  follows, new items keep it on the newest. `cursor.up` (count n) leaves the
  follow mode and moves to the n-th older item (clamped to the oldest);
  `cursor.down` moves to newer items and returns to follow mode when it
  reaches the newest. `cursor.first` goes to the oldest loaded item,
  `cursor.last` to the newest (follow mode). Half pages use the timeline's
  last viewport like the room list. The cursor is anchored to its item's
  `ItemRef`, so diffs that insert older items, or replace a local echo by
  the remote event (the SDK keeps the item's position), do not move it to
  another message; if its item disappears, it moves to the nearest remaining
  item at or after the old index, else the newest.
- Cursor actions act on the timeline when it has focus (they act on the room
  list as before when that has focus).
- Pagination (design): when the cursor is within 10 items of the oldest
  loaded item (counting items, not entries) and the start of the room has not
  been reached and no request is in flight for the room, return
  `PaginateBackwards(room)` once. This is checked after cursor moves and
  after every `Msg::Timeline`. `PaginationDone` clears the in-flight flag and
  records `reached_start`; `PaginationFailed` clears it and shows the error.
- Read receipts (design): after every message that can change the
  condition (`Msg::Timeline`, `Msg::Viewport` for the timeline,
  `Msg::TerminalFocus`, opening a room), if a room is open, the terminal is
  focused, and the timeline's last viewport includes the newest item, and the
  newest item that has a `Remote` id differs from the last one a receipt was
  requested for, return `SendReceipt { room, event }` and clear the room's
  counts optimistically.
- `message.view`: with the cursor on an item, `UiEffect::OpenView(ViewContent::Text(plain))`
  where `plain` is `content.plain_text()`; the view is open (`mode_label`
  `VIEW`; keys use `InputContext::MessageView` while its editor is in normal
  or visual mode and `EditorPassthrough` otherwise). `view.close`:
  `UiEffect::CloseView`, the view is closed.
- `message.yank`: `UiEffect::Yank(plain)` of the item under the cursor.
- `message.goto_reply`: if the item under the cursor replies to an event
  that is loaded, move the cursor there (leaving follow mode); otherwise the
  status `The replied-to message is not loaded`.
- Search in the timeline (Task 7 of phase 1): case-insensitive substring on
  `content.plain_text()`, starting after the cursor (towards newer items),
  wrapping, same messages as the room list search.
- Without an item under the cursor, these actions do nothing.

**Tests** (in the style of the phase 1 app tests; build entries with small
helpers): the cursor follows new items; `k` leaves follow mode and stays on
its item while new items arrive and older items are inserted; `G` returns to
follow mode; dividers and the read marker are skipped; pagination is
requested when the cursor nears the oldest item, only once while in flight,
again after `PaginationDone { reached_start: false }`, never after
`reached_start: true`; a failed pagination shows the error and allows a new
request; the receipt is requested once per newest remote event, only with
the terminal focused and the newest item in the viewport, and clears the
counts; `message.view` and `view.close` effects and mode; `yy` yields the
plain text; `gd` jumps to a loaded reply target and reports a missing one;
timeline search wraps; diffs for another room are ignored; opening another
room resets the timeline.

- [ ] Steps: failing tests, see them fail, implement, pass, fmt and clippy, commit
  ("Add the timeline state to the core: cursor, pagination, receipts, the message view, yank and reply jumps").

---

### Task 3: HTML bodies to rich text

**Files:**
- Modify: root `Cargo.toml` (`ruma = { version = "<the ruma version matrix-sdk 0.19 uses>", default-features = false, features = ["html-matrix"] }`,
  feature-only, as the design says; check with `cargo tree -i ruma` that one
  ruma is in the tree), `crates/mxt-matrix/Cargo.toml`
- Create: `crates/mxt-matrix/src/html.rs`

**Interfaces:**

```rust
/// Renders a `formatted_body` (org.matrix.custom.html) into `RichText`.
pub fn render_html(html: &str) -> RichText;
```

**Rules** (design, "Matrix adapter"): parse with `ruma::html::Html::parse`
and walk the tree. `<mx-reply>` is dropped. `<p>` and `<div>` are
paragraphs; `<br>` a line break; `<strong>`/`<b>` bold, `<em>`/`<i>` italic,
`<del>`/`<s>`/`<strike>` strikethrough, `<code>` inline code,
`<span data-mx-spoiler>` spoiler; `<pre><code class="language-x">` a code
block with language `x`; `<blockquote>` a quote; `<ul>`/`<ol start>` lists;
`<a href="https://matrix.to/#/@user:server">` a user mention (and
`#alias`/`!room` links a room mention), other `<a href>` links;
`<img data-mx-emoticon alt>` its alt text as `Emoticon`; `<h1>`–`<h6>`
bold paragraphs; unknown tags keep their text. Whitespace: collapse runs of
whitespace in text outside `<pre>` like a browser.

**Tests:** one per rule above with small HTML strings, plus a Discord-bridge
style body (`<p>hello <a href="https://matrix.to/#/@discord_1:example.org">Alice</a></p>`),
a reply fallback that is dropped, and malformed HTML that still yields its
text.

- [ ] Steps: failing tests, see them fail (under the lock), implement, pass, fmt and clippy, commit
  ("Render HTML message bodies into core rich text with ruma's HTML parser").

---

### Task 4: Timeline subscription in the matrix adapter

**Files:**
- Create: `crates/mxt-matrix/src/timeline.rs`
- Modify: `crates/mxt-matrix/src/adapter.rs`

**Behavior:**
- `SubscribeTimeline(room)`: build `room.timeline().await` (matrix-sdk-ui),
  keep at most one, send its initial items as `TimelineDiff::Reset`, then
  forward every batch of `subscribe()` diffs as one `Msg::Timeline`.
  `UnsubscribeTimeline` drops it (and its task).
- Conversion of `TimelineItem`s: virtual date dividers to `DateDivider`, the
  read marker to `ReadMarker`, events to `Item` when they are messages
  (`MsgLikeKind::Message`, stickers, redacted, unable to decrypt) and
  `Hidden` otherwise. `Item` fields: `ItemRef::Local(txn)` or
  `Remote(event_id)` from the item's identifier; sender and its display name
  (`sender_profile`, falling back to the user ID); timestamp; content
  (`formatted_body` through `render_html` when the format is
  `org.matrix.custom.html`, otherwise `RichText::plain(body)`; images and
  files with name, size, dimensions and MIME type); the reply preview from
  `in_reply_to` (sender display name and plain body when the details are
  ready); reactions grouped by key with the count and whether the own user
  is among the senders; `edited` when the item has a replacement; the send
  state for local echoes.
- `PaginateBackwards(room)`: `timeline.paginate_backwards(20)`, then
  `Msg::PaginationDone { reached_start }`, or `PaginationFailed`.
- `SendReceipt { room, event }`: a receipt of the configured kind
  (`ReceiptKind`) for `event` and the fully read marker
  (`room.send_multiple_receipts(Receipts::new().fully_read_marker(..).public_read_receipt(..))`
  or `private_read_receipt`).
- Tests: conversion from constructed values where the SDK types allow it;
  otherwise the pure helpers (reaction grouping, content mapping from plain
  fields). The SDK is not mocked.

- [ ] Steps: tests, implement, pass (under the lock), fmt and clippy, commit
  ("Subscribe the open room's timeline and forward its items, pagination and receipts").

---

### Task 5: Timeline pane, message view and clipboard in the terminal adapter

**Files:**
- Create: `crates/mxt-tui/src/timeline_view.rs`, `crates/mxt-tui/src/message_view.rs`,
  `crates/mxt-tui/src/clipboard.rs`
- Modify: `crates/mxt-tui/src/view.rs`, `crates/mxt-tui/src/lib.rs`,
  root `Cargo.toml` (`edtui = { version = "0.11", default-features = false,
  features = ["system-editor"] }`, `jiff = "1"`)

**Behavior:**
- `ui.time_format` check: `mxt-tui` exports `pub fn check_time_format(format: &str) -> Result<(), String>`
  (formats a fixed timestamp with `jiff::fmt::strtime`), and the configuration
  loader in `mxt` calls it at load time with the value's span
  (`dto::Ui::time_format` becomes `Spanned<String>`), so an invalid directive
  or an empty format is reported with its line before the UI starts (design:
  a typo is never silently ignored). Tests for a valid and an invalid format.
- Timeline pane: items oldest at the top, newest at the bottom; each item has
  a header (local time with `ui.time_format`, jiff, and the sender name) and
  the body: `RichText` mapped to ratatui lines (bold, italic, crossed out,
  code reversed or dim, spoilers as `█` blocks of the same width until the
  message view, links underlined with the URL kept for `gx` later, mentions
  bold), wrapped by display width; reply previews as a dim `> name: body`
  line; reactions as `key count` chips with the own reaction bold; `(edited)`
  dim; send state (`sending…`, `failed`). Notices dim; emotes `* name body`.
  Images `[image: <name> <w>x<h>]`, files `[file: <name> <size>]`, stickers
  `[sticker: <body>]`, redacted `(deleted)`, undecryptable `(unable to
  decrypt)`. Date dividers as a centered date line; the read marker as a
  labeled rule. Hidden entries are not drawn.
- Scrolling keeps the cursor item visible; in follow mode the newest item is
  at the bottom. An item taller than the pane is cut with `[+N lines]`. The
  cursor item is marked (a bar in the left column, reversed while the
  timeline has focus). The first and last visible entry indices are reported
  with `Msg::Viewport { pane: Timeline, .. }` when they change.
- Message view: `UiEffect::OpenView(Text)` opens a full-screen edtui editor
  (vim mode) with the text; keys reach it through `ReplayKeys`, and `mxt-tui`
  reports its mode with every key (`Msg::Key { editor: Some(mode) }`). Edits
  are discarded on `CloseView`. The terminal cursor is set to the editor's
  `cursor_screen_position()` (phase 0 finding: needed for input method
  placement).
- Clipboard (design, "Clipboard and terminal focus"): one
  `Rc<RefCell<Register>>` shared by every edtui state; edtui's
  `ClipboardTrait` writes OSC 52 (base64) and the register, and reads the
  register. `UiEffect::Yank` does the same.
- Verify by running (as in phase 1 Task 9, with a throwaway example feeding
  made-up timeline diffs): wrapping of long Japanese lines, a tall message
  with `[+N lines]`, the message view with `/` search and `yy`, OSC 52
  reaching tmux (`tmux show-buffer` after yanking, with `set-clipboard on`).

- [ ] Steps: input-conversion and clipboard unit tests (OSC 52 encoding),
  implement, verify by running, fmt and clippy, commit
  ("Draw the timeline, the message view and the clipboard in the terminal adapter").

---

### Task 6: Wire the timeline and check against the account

- `mxt` routes the new effects (already generic) and messages; run mxt
  against the account (the orchestrator does this) and check: a bridged room
  reads correctly, back pagination loads history, undecryptable messages
  decrypt after backup download, receipts are private with the development
  configuration, the message view shows a long message in full.

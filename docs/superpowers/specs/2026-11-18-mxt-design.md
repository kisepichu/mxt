# mxt design

Date: 2026-11-18
Status: draft for review (revision 2)

## Summary

mxt is a terminal Matrix client with nvim-like keybindings. It targets
accounts where most rooms are bridged from other networks (Discord, Slack,
and similar), so it shows which network a room belongs to, makes unread state
easy to navigate, and lets users rebind every key.

mxt is a single Rust binary. End-to-end encryption, sync, event aggregation
and unread counting come from matrix-rust-sdk (`matrix-sdk` and
`matrix-sdk-ui`). mxt itself is the user interface and the glue around the
SDK.

## Goals

- Read and write in encrypted rooms, including rooms bridged by mautrix
  bridges that only accept cross-signed devices.
- Show bridged rooms as `<network> / <room>` (for example
  `myserver / #general`), so rooms with the same name on different networks
  can be told apart.
- Show unread state per room, including unread messages in muted rooms, and
  let keybindings jump between rooms by unread state.
- Default keybindings follow common nvim conventions. Every binding can be
  changed in the configuration file.
- Compose messages with vim-style modal editing.
- Keep the domain and application logic pure and testable without a terminal
  or a homeserver.

## Non-goals

- Implementing any part of the Matrix protocol or end-to-end encryption.
  matrix-rust-sdk owns these.
- Running as a daemon or offering a remote API. mxt starts when the user runs
  it and exits when the user quits.
- Multiple accounts in one process.
- Being a general text editor. Message editing uses an embedded editor widget
  plus an external editor for long text.

## Scope of the first release

Included:

1. Password login and session restore. Entering the recovery key once per
   device to obtain cross-signing and key backup.
2. A room list that shows network names and unread badges.
3. A fuzzy room picker, also used to list rooms with unread mentions.
4. A timeline that renders HTML message bodies and shows replies, edits and
   reactions.
5. Sending messages written in Markdown, replies, and read receipts.
6. Toggling reactions, editing and deleting (redacting) the user's own
   messages.
7. Muting and unmuting a room.
8. Files: downloading and opening attachments, uploading a file, and image
   previews where the terminal supports them.
9. A keymap and action registry that lets the configuration bind any action,
   including "open the next unread room".

Not included in the first release:

- Threads, desktop notifications, typing notifications.
- Event-level mention lists (see "Mentions").
- SSO or OAuth login, logout, and account management.
- Setting up cross-signing or secret storage for an account that has none
  (bootstrapping needs user-interactive authentication). Such accounts must be
  set up in another client first.
- Themes and colors beyond the terminal's 16-color palette.
- Reloading the configuration at runtime.
- User-defined command aliases and a generic `:<action>` command.
- Prebuilt release binaries.

## Conventions

- All documents, code, comments and commit messages in the repository are in
  English.
- License: MIT. Code adapted from Apache-2.0 projects keeps its Apache-2.0
  notice in the adapted files, and the repository then ships `LICENSE-APACHE`.
- Dependency licenses are checked by `cargo-deny` in CI. GPL-family licenses,
  including the AGPL, are rejected.
- Tests and fixtures use made-up identifiers (`@alice:example.org`,
  `!room:example.org`). Real user IDs, room IDs, access tokens and logs are
  never committed.
- Access tokens are never logged. Message bodies are logged only at the
  `trace` level.

## Architecture

### Process and data flow

mxt is one process on a tokio runtime. crossterm handles terminal input and
output, and ratatui draws the screen.

```mermaid
flowchart LR
  input[terminal input] --> keymap
  keymap -->|Action| update["App::update"]
  mtask["matrix adapter (matrix-sdk-ui)"] -->|Msg| update
  tui[tui adapter] -->|Msg| update
  update -->|Effect::Matrix| mtask
  update -->|Effect::Ui| tui
  update --> state[(App state)]
  state --> render[ratatui render]
```

- The keymap turns key input into `Action`s. `App::update` applies actions and
  messages to the application state and returns `Effect`s.
- `update` never performs I/O and never reads a clock. Effects are plain data.
  The matrix adapter executes matrix effects (send a message, subscribe to a
  timeline, mark a room as read) and reports results and SDK stream updates
  back as `Msg`s. The tui adapter executes UI effects (load text into the
  composer, start a timer, open a file) and reports terminal facts as `Msg`s
  (submitted text, viewport sizes, terminal focus, timer expiry).
- Every frame is rendered from the core state plus the widget state that
  `mxt-tui` owns (the edtui editors and the image cache).
- Actions are resolved against the state, not against rendered widgets, so an
  action such as "open the next unread room" works the same whether or not a
  list has been drawn yet.

### Crates and the dependency rule

The repository is a Cargo workspace. Dependencies point inward, and Cargo
enforces it.

```mermaid
flowchart BT
  matrix["mxt-matrix: SDK adapter"] --> core
  tui["mxt-tui: terminal adapter"] --> core
  bin["mxt: binary, composition root"] --> matrix
  bin --> tui
  core["mxt-core: domain and application"]
```

| Crate | Responsibility | May depend on |
|---|---|---|
| `mxt-core` | Domain types, keymap and its resolver, action registry, pickers, `App` state and `update` | `std` and small pure crates without I/O (`nucleo-matcher`, `emojis`, `unicode-segmentation`). Not the SDK, `ruma`, an async runtime, a terminal library, `serde` or `toml`. |
| `mxt-matrix` | Builds the SDK client, converts SDK data into domain types, executes matrix effects, forwards SDK streams as `Msg`s, renders HTML bodies into domain rich text | `mxt-core`, `matrix-sdk`, `matrix-sdk-ui`, `ruma-html`, tokio |
| `mxt-tui` | Layout and widgets, input routing, the composers (edtui), image previews, OSC 52 clipboard, local time formatting, executing UI effects | `mxt-core`, ratatui, crossterm, edtui, ratatui-image, jiff |
| `mxt` | Command line, startup flow, configuration file loading (TOML DTOs), paths, the data directory lock, logging, wiring the adapters and the event loop | all of the above, `serde`, `toml`, `tracing` |

Because `mxt-core` does not depend on the SDK, its tests build and run without
compiling matrix-rust-sdk, and SDK upgrades only touch `mxt-matrix`.

### Functional core

Workflows are functions over domain types:

```rust
impl App {
    /// Deterministic and free of I/O. Mutates the state in place to avoid
    /// cloning the whole state for every message.
    pub fn update(&mut self, msg: Msg) -> Vec<Effect> { /* ... */ }
}

pub fn resolve_room(rooms: &RoomList, current: Option<&RoomId>, target: &RoomTarget) -> Option<RoomId> {
    /* ... */
}
```

Rules:

- Illegal states are unrepresentable where it is cheap to do so. Examples:

  ```rust
  /// A timeline item: a local echo before the server assigns an event ID,
  /// or a remote event. Mirrors the SDK's `TimelineEventItemId`.
  pub enum ItemRef {
      Local(TxnId),
      Remote(EventId),
  }

  pub enum Draft {
      New,
      Reply { to: ItemRef },
      Edit { target: OwnEvent },
  }

  /// An item sent by the current user. The sender is taken from the item
  /// itself, so a caller cannot pass the wrong one.
  pub struct OwnEvent(ItemRef);

  impl OwnEvent {
      pub fn new(item: &TimelineItem, me: &UserId) -> Option<Self> {
          (item.sender == *me).then(|| Self(item.id.clone()))
      }
  }

  /// Which room `room.open` and `picker.rooms` act on.
  pub enum RoomTarget {
      Pick { filter: RoomFilter, pick: Pick },
      Fuzzy { filter: RoomFilter, query: String },
  }
  ```

  Replying and editing at the same time, editing another user's message, and
  a `room.open` that has both a pick rule and a query cannot be expressed.
- Parse, don't validate. Configuration values and SDK data are converted into
  domain types once, at the boundary. Code inside the core trusts them.
- Identifiers in the core are mxt's own newtypes (`RoomId`, `EventId`,
  `TxnId`, `UserId`), not `ruma` types. Timestamps are UTC milliseconds.
- Subscriptions and timers are effects too: `Effect::SubscribeTimeline`,
  `Effect::UnsubscribeTimeline`, `Effect::PaginateBackwards` and
  `Effect::StartTimer { id, after }`. Their results arrive as `Msg`s
  (timeline diffs, `Msg::Timeout { id }`).
- No ceremony: no repository traits, dependency injection containers or one
  type per use case. Types and functions are enough.

### Domain model

The main types in `mxt-core`:

- `RoomSummary`: id, `RoomName { network: Option<String>, room: String }`,
  kind (direct message or group), `UnreadCounts`, `NotificationMode`
  (default, mentions and keywords only, mute), time of the latest activity.
  Spaces are not room summaries; their names only feed `RoomName.network`.
- `UnreadCounts { messages, notifications, mentions, marked }`. `messages`
  counts unread messages even in muted rooms. `notifications` counts unread
  events that notify according to the user's push rules. `mentions` counts
  unread highlights. `marked` is the manual unread mark. All counts are
  computed client-side by the SDK (see "Risks").
- `Badge`, derived from `UnreadCounts` (see "Unread badges").
- `RoomTarget`, `RoomFilter` and `Pick` (see "Action registry").
- `TimelineItem`: `id: ItemRef`, sender, timestamp, `MessageContent`,
  optional reply preview, reaction groups (key, count, whether the user
  reacted), an edited flag, and the local send state (sending, sent, failed).
  The timeline also contains date dividers and the read marker, which the SDK
  provides as virtual items.
- `MessageContent`: `Text(RichText)`, `Notice(RichText)`, `Emote(RichText)`,
  `Image { name, size, dimensions }`, `File { name, size, mime }` (also used
  for video and audio), `Sticker { body }`, `Redacted`, `Undecryptable`,
  `Unsupported { kind }`.
- `RichText`: a list of blocks (paragraph, code block with optional language,
  block quote, ordered or unordered list). Paragraphs hold inline spans: text
  with bold, italic, strikethrough, inline code or spoiler style; links with a
  URL; mentions of users or rooms; custom emoticons shown by their alt text.
  The terminal adapter maps it to ratatui lines.
- Drafts: one `Draft` per room, kept when switching rooms, plus whether that
  room's composer is empty.
- UI state: focus (room list, timeline, composer), mode, keymap resolver
  state, picker state, cursor positions, the last known viewport of each pane,
  terminal focus, the status message.

### Keymap resolver

The resolver lives in `mxt-core` and is deterministic, so its behavior is
covered by unit tests.

- It holds the pending key sequence. Count prefixes (`5j`) are part of the
  same pending state.
- A key sequence that is a complete binding and not a prefix of a longer one
  runs immediately.
- A key sequence that is a prefix of a longer binding stays pending. The
  resolver returns `Effect::StartTimer` with `ui.timeoutlen` (default 1000 ms,
  like nvim's `timeoutlen`).
- On `Msg::Timeout`: if the pending sequence is itself a binding, it runs.
  Otherwise the pending keys are released (see below).
- When the next key makes the sequence match nothing: a pending complete
  binding runs first, and the new key is resolved afresh. Without a complete
  binding, the pending keys are released.
- Released keys go to the text input that would have received them: the
  composer's editor through `Effect::Ui(ReplayKeys(keys))`, or the picker or
  command line query. In panes without text input they are dropped.

### Matrix adapter

- Sync: `SyncService` with simplified sliding sync (MSC4186) and
  `RoomListService` for the set of rooms.
- Timelines: only the open room has a subscribed `Timeline`. Opening another
  room unsubscribes the previous one. The core requests
  `Effect::PaginateBackwards` when the timeline cursor comes within 10 items
  of the oldest loaded item, with at most one request in flight per room.
- Room summaries come from the SDK room info: display name,
  `num_unread_messages`, `num_unread_notifications`, `num_unread_mentions`,
  `is_marked_unread`, the user-defined notification mode and the latest event
  time. The core filters and sorts them itself, so the meaning of every filter
  is defined in one place.
- Bridge state: `RoomListService` requests a fixed set of state events that
  does not include `m.bridge`, and it has no API to extend that set. mxt runs
  a second sliding sync connection (`client.sliding_sync("mxt-bridge-state")`)
  with one list over all rooms, `required_state` of `("m.bridge", "*")` and
  `("uk.half-shot.bridge", "*")`, and `timeline_limit` 0. Its responses go
  through the SDK into the same state store. `SpaceParent` state comes with
  the room list connection already.
- Network names, first match wins:
  1. `network.displayname` of the room's `m.bridge` event (or
     `uk.half-shot.bridge`)
  2. `protocol.displayname` of that event
  3. the name of the room's canonical parent space
  4. the name of the parent space, when the user has joined exactly one
  5. none
- Recovery: `Recovery::recover(recovery_key)`.
- Mute: the SDK notification settings. Muting sets the room mode to mute.
  Unmuting removes the user-defined room rule, so the account default applies
  again.
- Sending: the composer text is Markdown and is sent with
  `RoomMessageEventContent::text_markdown` (matrix-sdk's `markdown` feature),
  which fills both `body` and `formatted_body`. Replies and edits use the
  timeline's reply and edit calls.
- Reactions: `Timeline::toggle_reaction`. Choosing a reaction the user already
  sent removes it.
- Read receipts: the core requests a receipt for the latest event when the
  room is open, the timeline shows its newest item, and the terminal has focus
  (crossterm focus events). The adapter sends the configured receipt type
  (`m.read` or `m.read.private`) and moves the `m.fully_read` marker to the
  same event. `room.mark_read` does the same for the selected room and also
  clears its manual unread mark.
- Optimistic clearing: when the core requests a receipt or runs
  `room.mark_read`, it sets that room's `UnreadCounts` to zero immediately.
  The next SDK update overwrites it. This makes action sequences such as
  `["room.mark_read", <open the next unread room>]` deterministic.
- Room keys are shared with the SDK's default recipient strategy
  (`CollectStrategy::AllDevices`), which does not refuse to send when another
  user's identity changes.
- HTML bodies: `formatted_body` is parsed with ruma-html into `RichText`.
  `<mx-reply>` fallbacks are dropped, `<del>` becomes strikethrough,
  `data-mx-spoiler` becomes spoiler style, and `<img data-mx-emoticon>`
  becomes its alt text.
- Sync path: phase 0 decides between sliding sync and classic `/sync` for the
  target homeservers, and only the chosen path is implemented. If classic sync
  is chosen, the adapter builds the room set from `Client::rooms()` and reads
  `m.bridge` from the full room state. Either way the core receives the same
  `Msg`s.

### Terminal adapter and the composer

- The composer is an edtui editor in vim mode, owned by `mxt-tui`, one editor
  state per room. edtui is used with
  `default-features = false, features = ["system-editor"]`, which leaves out
  the system clipboard (arboard), mouse support and syntax highlighting.
- edtui's clipboard is a custom `ClipboardTrait` implementation: copying
  writes the text to the terminal clipboard with OSC 52 and keeps it in an
  internal register, and pasting reads the internal register. Timeline yanks
  use the same register.
- Text editing is an input detail. The core only knows the room's `Draft` and
  whether its composer is empty, which the tui adapter reports with
  `Msg::ComposerEdited { room, empty }` when that changes.
- Sending: `composer.send` makes the tui adapter read the editor text and send
  `Msg::Submit { room, body }`. If the body is blank, nothing happens.
  Otherwise the core returns the send effect and
  `Effect::Ui(ClearComposer(room))`, and the room's draft returns to `New`.
  Failed sends are shown on the local echo (see "Error handling").
- Editing an existing message: `message.edit` requires an empty composer and
  otherwise reports "Clear the composer first" in the status line. On success
  the core returns `Effect::Ui(LoadComposer { room, text })`.
- `composer.cancel`: a reply draft becomes `New` and keeps its text. An edit
  draft becomes `New` and the composer is cleared.
- Input routing while the composer has focus:
  - edtui in normal or visual mode: look the key up in `composer.normal` or
    `composer.visual`, then in `normal`. If it is not bound, pass it to edtui.
  - edtui in insert mode: look the key up in `composer.insert`. If it is not
    bound, pass it to edtui.
  - edtui in search mode: every key goes to edtui, including `<Esc>`.
- The external editor (`composer.external_editor`) uses edtui's
  `system-editor` feature, which opens `$VISUAL` or `$EDITOR`.
- The tui adapter reports `Msg::Viewport { pane, first, last }` after a render
  changes which items are visible. Half-page movement moves the cursor by half
  of the visible item count (at least one). The core never computes text
  layout.
- Replacing edtui later (with another editor widget or an in-house vim engine)
  only touches `mxt-tui`.

## Startup

Command line: `mxt [--config PATH]` and `mxt --version`.

1. Load the configuration. If the file does not exist, print its path and a
   minimal example, and exit with a non-zero status.
2. Take an exclusive lock on `$XDG_DATA_HOME/mxt/lock` (`std::fs::File::try_lock`).
   If another mxt holds it, exit with a non-zero status. Two processes must
   never share the crypto store.
3. If a session file exists, restore the session. Otherwise ask for the
   password on the terminal without echo, log in (no refresh token is
   requested, so the stored access token does not change), and save the
   session.
4. Right after a first login, ask for the recovery key. An empty input skips
   it; `:recover` asks again later. If the account has no secret storage, mxt
   prints that recovery has to be set up in another client and continues.
5. Start the terminal UI and sync.

If the homeserver answers `M_UNKNOWN_TOKEN` at any time, mxt restores the
terminal, renames the data directory to `mxt.invalid-<timestamp>`, prints why,
and exits. The next start logs in as a new device with fresh stores.

## User interface

### Layout

- Left: the room list, which can be hidden.
- Right: the timeline of the open room, with the composer below it.
- Image preview pane: shown on the right of the timeline while the selected
  message is an image.
- Bottom line: status line showing the mode, the open room and the sync state.
  Pressing `:` turns it into the command line.
- Overlays: the fuzzy room picker and the emoji picker.

### Modes

| Mode | When | Keys |
|---|---|---|
| `NORMAL` | the room list or the timeline has focus | keymap |
| composer modes | the composer has focus; the status line shows edtui's mode (`INSERT`, `NORMAL`, `VISUAL`, `SEARCH`) | see "Input routing" above |
| `COMMAND` | the `:` command line is open | `command_line` table, then text input |
| `PICKER` | the room picker or the emoji picker is open | `picker` table, then text input |
| `CONFIRM` | an action asks for confirmation | `y` confirms, any other key cancels |
| `SECRET` | the recovery key is being entered | masked text input, `<CR>` submits, `<Esc>` cancels |

In `NORMAL`, the room list and the timeline have no character cursor. They
move one room or one message at a time. Because of that, `<Space>` is free
and is the default leader.

### Room names

- Rooms with a network name (see "Matrix adapter"): `<network> / <room>`.
- Direct messages: the other user's display name, prefixed with the
  protocol name when the room is bridged (`Discord / Alice`).
- Other rooms: the room's display name.

### Unread badges

A room shows at most one badge, chosen by the first rule that matches:

| Rule | Badge | Style |
|---|---|---|
| `mentions > 0` | `@<mentions>` | mention style |
| `notifications > 0` | `<notifications>` | bold |
| `messages > 0` | `<messages>` | dim (typical for muted rooms) |
| `marked` | `U` | marked style |

Counts above 99 show as `99+`. Muted rooms also show a `Muted` label, and
rooms set to mentions and keywords only show a `Mentions only` label.

### Room list

The default order is by latest activity, newest first. `sort = "name"` sorts
case-insensitively by the full displayed name (`network / room`), which groups
rooms by network. Spaces themselves are not listed.

### Timeline

- State events (membership changes, name or topic changes) are not shown in
  the first release.
- Date dividers and the user's read marker come from the SDK's virtual items.
- Messages the SDK cannot decrypt yet show a placeholder. Redacted messages
  show `(deleted)`.
- Images show `[image: <name> <width>x<height>]` inline. The preview itself
  appears in the preview pane (see "Files and images").

### Default keymap

| Table | Key | Action | nvim analogy |
|---|---|---|---|
| `normal` | `:` | `command_line.open` | same |
| `normal` | `<leader><leader>` | `picker.rooms` | file picker in common distributions |
| `normal` | `<leader>e` | `sidebar.toggle` | file explorer toggle |
| `normal` | `<leader>m` | `{ action = "picker.rooms", filter = "mentions" }` | |
| `normal` | `<C-w>h` / `<C-w>l` | `focus.room_list` / `focus.timeline` | window movement |
| `room_list`, `timeline` | `j` / `k` | `cursor.down` / `cursor.up` | same |
| `room_list`, `timeline` | `gg` / `G` | `cursor.first` / `cursor.last` | same |
| `room_list`, `timeline` | `<C-d>` / `<C-u>` | `cursor.half_page_down` / `cursor.half_page_up` | same |
| `room_list`, `timeline` | `/` / `n` / `N` | `search.start` / `search.next` / `search.prev` | same |
| `room_list` | `<CR>` | `room.open_selected` | |
| `timeline` | `i` / `a` | `composer.focus` | insert |
| `timeline` | `r` | `message.reply` | |
| `timeline` | `e` | `message.edit` (own messages only) | |
| `timeline` | `dd` | `message.delete` (own messages only, after confirmation) | delete line |
| `timeline` | `yy` | `message.yank` | same |
| `timeline` | `+` | `message.react` (opens the emoji picker) | |
| `timeline` | `gx` | `message.open` (link or attachment) | `gx` |
| `timeline` | `gd` | `message.goto_reply` (the replied-to message) | go to definition |
| `composer.insert` | `<CR>` | `composer.send` | |
| `composer.insert` | `<C-j>` | `composer.newline` | |
| `composer.insert`, `composer.normal` | `<C-x><C-e>` | `composer.external_editor` | shell edit-command-line |
| `composer.normal` | `<Esc>` | `focus.timeline`; the draft is kept | |
| `composer.normal` | `<C-c>` | `composer.cancel` | |
| `picker` | `<C-n>` / `<C-p>` | `picker.next` / `picker.prev` | completion menu |
| `picker` | `<CR>` / `<Esc>` | `picker.accept` / `picker.cancel` | |
| `command_line` | `<CR>` / `<Esc>` / `<Tab>` | `command_line.execute` / `command_line.cancel` / `command_line.complete` | same |

`<Esc>` in the composer's insert mode is not bound: edtui switches to its
normal mode and the draft is kept.

`<C-j>` inserts a newline because many terminals cannot tell Shift+Enter apart
from Enter.

There is no default binding for "open the next unread room". The action exists
and users bind it themselves, for example
`"]u" = { action = "room.open", filter = "unread", pick = "next" }`.

### Search and jumps

- `/` searches the loaded items of the focused pane, case-insensitively, by
  substring: display names in the room list, plain-text bodies in the
  timeline. There is no server-side search. `n` and `N` wrap around.
- `gd` jumps to the replied-to message if it is loaded. Otherwise the status
  line says that it is not loaded.

### Commands

Commands resolve through the same action registry as keys. Everything after
the command name, trimmed, is one argument, so `:open my room` and paths with
spaces work without quoting.

| Command | Action |
|---|---|
| `:open <room>` | `room.open` with a fuzzy query, with completion over room names |
| `:mute` / `:unmute` | `room.mute` / `room.unmute` |
| `:upload <path>` | `file.upload` |
| `:recover` | `account.recover` (asks for the recovery key in `SECRET` mode) |
| `:q` | `app.quit` |

### Mentions

`<leader>m` opens the room picker filtered to rooms whose `mentions > 0`.
Listing individual mention events needs the server's `/notifications`
endpoint, and phase 0 checks whether the target homeservers support it.

### Files and images

- `gx` on an attachment downloads it to `files.download_dir` and runs
  `files.opener` with the file path appended. If a file with the same name
  exists, the download is saved as `name (1).ext`, `name (2).ext`, and so on.
- `:upload <path>` sends the file as an image or a file message, using the
  MIME type guessed from the file.
- Image previews use ratatui-image, which detects the terminal's graphics
  protocol (sixel, kitty, iTerm2) and falls back to half blocks. The preview
  pane shows the server thumbnail of the selected image message. Previews are
  not drawn inline in the scrolling timeline. `ui.image_preview = "off"`
  disables the pane.

### Clipboard

`message.yank` and edtui copies use OSC 52. Inside tmux, OSC 52 reaches the
outer terminal only with `set -g set-clipboard on`. The user documentation
says so.

## Configuration

### Location

`$XDG_CONFIG_HOME/mxt/config.toml`, or `~/.config/mxt/config.toml` when
`XDG_CONFIG_HOME` is unset. The same path is used on macOS.

### Example

```toml
[account]
user_id = "@alice:example.org"
# homeserver = "https://matrix.example.org"  # omitted: discovered via .well-known

[ui]
leader = "<Space>"
timeoutlen = 1000                                 # milliseconds
room_list = { visible = true, sort = "recent" }  # sort: "recent" | "name"
time_format = "%H:%M"                             # strftime, in the local time zone
image_preview = "auto"                            # "auto" | "off"
read_receipts = "public"                          # "public" | "private"

[files]
download_dir = "~/Downloads"
opener = ["xdg-open"]           # for example ["wslview"] on WSL

[keymap.normal]                 # outside text input, in every pane
"]u" = { action = "room.open", filter = "unread", pick = "next" }
"<leader>x" = ["room.mark_read", { action = "room.open", filter = "unread", pick = "next" }]

[keymap.timeline]               # only while the timeline has focus
"dd" = "nop"                    # remove a default binding
"D" = "message.delete"

[keymap.composer.insert]        # checked before keys reach the editor
"<C-s>" = "composer.send"
```

Settings:

- `ui.read_receipts = "public"` sends `m.read` receipts, which other users
  see. `"private"` sends `m.read.private`, which only the user's own devices
  see.
- `ui.time_format` is applied in the local time zone (jiff, in `mxt-tui`).
- `files.download_dir` defaults to `$XDG_DOWNLOAD_DIR` when set and to
  `~/Downloads` otherwise. A leading `~` is expanded when the configuration
  is loaded.
- `files.opener` is a command and its arguments. It defaults to `["xdg-open"]`
  on Linux and `["open"]` on macOS.

### Binding values

- A string names an action without arguments: `"picker.rooms"`.
- A table names an action with arguments:
  `{ action = "room.open", filter = "unread", pick = "next" }`.
- An array runs several actions in order against the state.
- `"nop"` removes a binding, like nvim's `<Nop>`.

### Tables and resolution

| Table | Used when | If the key is not bound |
|---|---|---|
| `room_list` | the room list has focus | look in `normal` |
| `timeline` | the timeline has focus | look in `normal` |
| `composer.normal`, `composer.visual` | the composer has focus and edtui is in normal or visual mode | look in `normal`, then pass the key to edtui |
| `composer.insert` | the composer has focus and edtui is in insert mode | pass the key to edtui |
| `picker` | a picker is open | type it into the query |
| `command_line` | the command line is open | type it into the command line |
| `normal` | fallback for the tables above that name it | ignore the key |

`normal` holds only bindings that make sense everywhere outside text input.
Movement keys are in `room_list` and `timeline`, so they reach edtui in the
composer. The user's tables are merged over the defaults key by key. Movement
actions accept a count prefix (`5j`).

### Action registry

Actions are named `<target>.<verb>`. The first release has:

- rooms: `room.open`, `room.open_selected`, `room.mark_read`, `room.mute`,
  `room.unmute`
- messages: `message.reply`, `message.edit`, `message.delete`,
  `message.yank`, `message.react`, `message.open`, `message.goto_reply`,
  `message.resend`
- composer: `composer.focus`, `composer.send`, `composer.newline`,
  `composer.cancel`, `composer.external_editor`
- pickers: `picker.rooms`, `picker.next`, `picker.prev`, `picker.accept`,
  `picker.cancel`
- command line: `command_line.open`, `command_line.execute`,
  `command_line.cancel`, `command_line.complete`
- movement and search: `cursor.down`, `cursor.up`, `cursor.first`,
  `cursor.last`, `cursor.half_page_down`, `cursor.half_page_up`,
  `search.start`, `search.next`, `search.prev`
- other: `sidebar.toggle`, `focus.room_list`, `focus.timeline`, `file.upload`,
  `account.recover`, `app.quit`, `nop`

Each action declares typed arguments. `room.open` takes a `RoomTarget`:

| Argument | Values | Meaning |
|---|---|---|
| `filter` | `all` (default) | every joined room |
| | `unread` | any badge: `messages`, `notifications` or `mentions` above zero, or `marked` |
| | `notify` | `notifications` or `mentions` above zero, or `marked` |
| | `mentions` | `mentions` above zero |
| `pick` | `first` (default) | the first matching room in the current room order |
| | `next` | the first matching room after the current room in the current room order, wrapping around; the first matching room when no room is open |
| | `prev` | the same, backwards |
| `query` | string | fuzzy name query; the best match among the rooms that pass `filter` |

`pick` and `query` are mutually exclusive. A binding with both is rejected
when the configuration is loaded. `picker.rooms` takes an optional `filter`
with the same values.

### Layers

- `mxt` reads the file into serde DTOs and converts argument values into the
  core's neutral `ArgValue` (string, integer, boolean).
- The core builds checked types with `Action::from_spec(name, args)`,
  `KeySeq::parse("<C-x><C-e>")` and `Keymap::new(..)`.
- Errors are reported with the file, line and reason before the terminal UI
  starts, and mxt exits with a non-zero status: unknown keys
  (`deny_unknown_fields`), unknown actions, wrong or conflicting arguments,
  invalid key notation. A typo is never silently ignored.

## Storage

| Path | Contents |
|---|---|
| `$XDG_DATA_HOME/mxt/` (mode 0700) | session file (mode 0600), the lock file, SDK state and crypto stores |
| `$XDG_CACHE_HOME/mxt/` | the SDK's event cache and media cache (not user downloads, which go to `files.download_dir`) |
| `$XDG_STATE_HOME/mxt/` | logs |

The usual defaults apply when the variables are unset (`~/.local/share`,
`~/.cache`, `~/.local/state`), on macOS as well.

The SQLite stores have no passphrase. They are protected by the directory and
file permissions. An OS keyring would add dependencies and differs between
platforms.

## Error handling

- Startup failures (configuration, lock, login, opening the stores) are
  printed before the terminal UI starts, and mxt exits with a non-zero status.
- Runtime failures of effects come back as `Msg::EffectFailed`. They are shown
  in the status line and logged. They never panic. The status line shows only
  the latest message, so the log is the place to look for earlier ones.
- Sync state (synced, offline, retrying) is shown in the status line. When the
  sync service stops with an error, mxt restarts it with backoff.
- Events that cannot be decrypted yet show a placeholder. The `Timeline`
  retries decryption when keys arrive.
- Failed sends keep the local echo with a failed marker. `message.resend` sends
  it again.
- Redaction asks for confirmation in `CONFIRM` mode because it cannot be
  undone.
- `M_UNKNOWN_TOKEN` is handled as described in "Startup".
- A panic hook restores the terminal before printing the panic.

## Logging

`tracing` writes daily files to `$XDG_STATE_HOME/mxt/` and keeps seven days.
`MXT_LOG` takes an `EnvFilter` directive. The default is `info` for mxt and
`warn` for matrix-rust-sdk.

## Testing

- `mxt-core` carries most tests, as plain unit tests:
  - state transitions in `update`, including drafts kept per room,
    `composer.cancel`, and the empty-composer rule for `message.edit`
  - key notation parsing, table resolution and fallbacks, pending sequences,
    timeouts and released keys
  - `Action::from_spec` rejecting unknown actions, wrong arguments and
    `pick` together with `query`
  - room filtering, ordering, `pick = next` wrapping around, and optimistic
    clearing before a following action
  - badge precedence and the network name order
  - `OwnEvent` construction
- `mxt-matrix`: tests for converting SDK values into domain types and for HTML
  rendering, with constructed inputs. The SDK is not mocked. The rest is
  covered by phase 0 and by running mxt against a homeserver.
- `mxt-tui`: no rendering snapshots, which would pin incidental layout. It is
  verified by running mxt.
- Continuous integration (GitHub Actions, Linux and macOS on Apple Silicon),
  with the flake's toolchain:
  - On every push and pull request, inside `nix develop` with the Cargo cache
    restored between runs: `cargo fmt --check`,
    `cargo clippy --all-targets -- -D warnings`, `cargo test`, and
    `cargo deny check licenses advisories`.
  - The package build (`nix build`, which also runs the tests) runs when
    `flake.nix`, `flake.lock`, `Cargo.lock` or a `Cargo.toml` changes, every
    week, and on demand. This keeps the full Nix build of matrix-rust-sdk out
    of every push.
- A prek pre-commit configuration runs `cargo fmt --check` and clippy.

## Build and distribution

### Toolchain

The Rust toolchain comes from the flake's pinned nixpkgs (`nixos-unstable`).
matrix-sdk 0.19 needs Rust 1.96 or later. `nix develop` provides cargo, rustc,
clippy, rustfmt, rust-analyzer and cargo-deny, so every machine and CI use the
same version. The version changes only with `nix flake update`. `Cargo.toml`
declares `rust-version = "1.96"` for builds outside Nix.

`[profile.dev]` reduces debug information of dependencies to
`line-tables-only`, which keeps the target directory smaller and linking
lighter.

### Nix flake

- Systems: `x86_64-linux`, `aarch64-linux`, `aarch64-darwin`. nixpkgs
  dropped `x86_64-darwin` in 2026.
- `packages.default`: `rustPlatform.buildRustPackage` with
  `cargoLock.lockFile = ./Cargo.lock`. `Cargo.lock` is committed. Git
  dependencies are avoided, so no output hashes need maintenance. Native
  inputs follow the SDK's needs (nixpkgs' iamb package, also built on
  matrix-rust-sdk, needs `sqlite`, `aws-lc` and `pkg-config`).
- `devShells.default`: the toolchain listed above.
- `checks.package`: the package build.

### Installing

- Nix: `nix profile install github:kisepichu/mxt`, or use the flake as an
  input of a NixOS or home-manager configuration.
- Without Nix: `cargo install --git https://github.com/kisepichu/mxt`, for
  example through mise's cargo backend.

Prebuilt binaries on GitHub Releases are out of scope for the first release.

## Development phases

| Phase | Work | Exit criteria |
|---|---|---|
| 0. Spike | The flake's development shell. A throwaway binary that logs in, recovers, runs `SyncService` and `RoomListService` plus the bridge state connection, and prints the room list with unread counts and network names and the latest message of a bridged room. A small edtui program for checking CJK text and input method input. The spike is deleted afterwards and its findings are added to this document. | Decisions recorded: (1) sliding sync or classic sync for the target homeservers; (2) the bridge state connection stores `m.bridge`, and the network name order yields names for bridged channels and DMs; (3) after a long offline period, a muted room's `messages` count is not zero and is close to the real number of unread messages; (4) whether `/notifications` is supported; (5) edtui handles CJK width and input method input correctly, or the composer choice is revisited. |
| 1. Skeleton | Workspace and crates, the flake package and checks, CI, the prek configuration, core domain (room summaries, filters, ordering, keymap and resolver, actions, `update`), startup (configuration, lock, login, session restore, recovery), sync, the room list pane and the status line | The room list with network names and unread badges can be browsed, keymap overrides from the configuration work, and CI passes |
| 2. Timeline | `Timeline` subscription, HTML rendering, replies, edits and reactions shown, date dividers and the read marker, back pagination, read receipts | Rooms can be read day to day |
| 3. Sending | The edtui composers, sending Markdown, replying, editing, deleting, toggling reactions with the emoji picker, the external editor | Messages can be written day to day |
| 4. Rest of the first release | Fuzzy room picker, mentions filter, mute and unmute, file download, open and upload, the image preview pane | Everything in "Scope of the first release" works |

## Risks and open questions

- **Sliding sync on bridge-heavy homeservers.** A homeserver can advertise
  `org.matrix.simplified_msc3575` and still behave differently from what
  `matrix-sdk-ui` expects. Phase 0 tests this before either sync path is
  built.
- **The bridge state connection.** A second sliding sync connection costs one
  more long poll. Phase 0 checks that its state reaches the store. If it does
  not, network names come from parent spaces only (steps 3 and 4 of the
  order).
- **Client-side unread counts.** The SDK counts unread messages from the
  events it has loaded since the user's read receipt. After a gap (a long
  offline period, or the small timeline limit of the room list), it paginates
  back to find the receipt, and until then the count can be too low. The
  server's `unread_notification_counts()` follow push rules and are zero in
  muted rooms, so they cannot replace it. Phase 0 measures this.
- **edtui and CJK text.** Cursor placement with wide characters and input
  method input are untested. Phase 0 tests them before the composer is built.
- **edtui differences from vim.** edtui has no count prefixes, and `r` is
  redo instead of replace. Rebinding through edtui's keybinding API covers
  what it can. The rest is accepted for the first release.
- **Mentions when sending.** Bridges turn Matrix mention pills and
  `m.mentions` into mentions on the remote network. Plain `@name` text may not
  notify anyone there. Whether the first release gets `@` completion that
  inserts pills is not decided yet.
- **Network names for other bridges.** Discord rooms carry the server name in
  `m.bridge`. Other bridges are expected to fill `network` or `protocol`
  similarly, which is not yet verified.
- **SDK API churn.** matrix-rust-sdk is pre-1.0, and upgrades can break its
  API. The adapter boundary keeps that work inside `mxt-matrix`.

## Third-party licenses

`cargo-deny` checks the full dependency tree. For reference, the direct
dependencies with notable licenses:

| Component | License |
|---|---|
| matrix-rust-sdk (`matrix-sdk`, `matrix-sdk-ui`), vodozemac | Apache-2.0 |
| ratatui, ratatui-image, edtui, ruma-html | MIT |
| `nucleo-matcher` | MPL-2.0 |
| `emojis` | (MIT OR Apache-2.0) AND Unicode-3.0 |
| jiff | Unlicense OR MIT |
| unicode-segmentation | MIT OR Apache-2.0 |

All of them can be used from an MIT-licensed program.

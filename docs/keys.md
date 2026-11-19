# Keys

mxt's keys follow nvim. `<leader>` is `<Space>` unless `ui.leader` says
otherwise. Every binding below can be changed or removed, and any action can
be bound; see [Configuration](configuration.md#keymap) for the tables, the
key notation and the actions.

## The screen

- The room list is on the left; `<leader>e` hides and shows it. The open
  room's timeline is on the right, with the composer below it.
- The status line at the bottom shows the mode, a count or keys typed so far,
  the open room, the last message or a question, and on the right the sync
  state: `starting` before the first sync, then `synced`, `offline` (no
  answer from the homeserver) or `retrying` (another error). mxt keeps
  trying in both cases. `:` and `/` turn it into the command line.
- Pickers (rooms, reactions, links), the mention menu above the composer, and
  the message view open over the panes.

A room in the list shows at most one badge, the first that applies:

| Badge | Meaning |
|---|---|
| `@3` | unread mentions of you |
| `3`, bold | unread messages that notify you |
| `3`, dim | unread messages that do not notify you, as in muted rooms |
| `U` | marked as unread |

Counts above 99 show as `99+`. Muted rooms also show `Muted`, and rooms set
to notify only for mentions and keywords show `Mentions only`.

In the timeline, each message has a header with its time, the sender, and
`(edited)`, `sending…` or `failed` when they apply. A reply's header is
followed by a row `> name: text` of the message it replies to, and
reactions follow the message as `👍 2` (yours in bold). Images show as
`[image: name 800x600]`, files as `[file: name 1.2 MiB]`, and also
`[sticker: …]`, `(deleted)`, `(unable to decrypt)` (the key may still
arrive) and `(unsupported: …)`.
A message taller than the pane ends with `[+N lines]`; `<CR>` shows it in
full. Date dividers and a `new messages` line, where you stopped reading,
come between messages.

## Modes

| Mode in the status line | When | Keys |
|---|---|---|
| `NORMAL` | the room list or the timeline has focus | the keymap |
| `INSERT`, `NORMAL`, `VISUAL`, `SEARCH` | the composer has focus; the mode of its editor | the keymap, then the editor (see [The editors](#the-editors)) |
| `COMMAND` | the `:` command line or a `/` search line is open | the `command_line` table, then the text |
| `PICKER` | a picker is open | the `picker` table, then the query |
| `VIEW NORMAL`, `VIEW INSERT`, `VIEW VISUAL`, `VIEW SEARCH` | the message view is open; the mode of its editor | the `message_view` table in normal and visual mode, then the editor |
| `CONFIRM` | an action asks first, as `dd` does | `y` confirms, any other key cancels |

In `NORMAL`, the room list and the timeline have no character cursor: the
cursor moves one room or one message at a time. That leaves `<Space>` free
for the leader.

Counts work in the room list and the timeline: `5j` moves five messages
down. A count applies to `j`, `k`, `<C-d>` and `<C-u>`; `0` only continues
a count that has started.

The command line, search lines and picker queries take typed text,
`<BS>` (on an empty command or search line, it closes the line), `<C-u>`
(clear) and `<C-w>` (delete the last word). Other keys do nothing there.

## Default keymap

`normal` applies wherever the focused table does not bind the key, outside
text input: in the room list, the timeline, and the composer in normal and
visual mode.

### `normal`

| Key | Action | |
|---|---|---|
| `:` | `command_line.open` | the command line |
| `<leader><leader>` | `picker.rooms` | the room picker |
| `<leader>m` | `picker.rooms` with `filter = "mentions"` | the room picker, rooms with unread mentions only |
| `<leader>e` | `sidebar.toggle` | hide or show the room list |
| `<C-w>h` | `focus.room_list` | focus the room list (and show it) |
| `<C-w>l` | `focus.timeline` | focus the timeline |

### `room_list` and `timeline`

| Key | Action | |
|---|---|---|
| `j` / `k` | `cursor.down` / `cursor.up` | next or previous room or message |
| `gg` / `G` | `cursor.first` / `cursor.last` | first or last |
| `<C-d>` / `<C-u>` | `cursor.half_page_down` / `cursor.half_page_up` | half a page |
| `/` | `search.start` | search the loaded rooms or messages |
| `n` / `N` | `search.next` / `search.prev` | repeat the search |

The timeline lists the oldest message at the top, so `j` and `n` go to newer
messages. Older messages load as the cursor nears the top. A search looks at
the room names in the room list and at the text of the loaded messages in the
timeline, ignoring case, and wraps around; an empty search repeats the last
one. There is no search on the server.

### `room_list`

| Key | Action | |
|---|---|---|
| `<CR>` | `room.open_selected` | open the room under the cursor |

### `timeline`

| Key | Action | |
|---|---|---|
| `i`, `a` | `composer.focus` | write in insert mode |
| `<CR>` | `message.view` | show the message in full, or the image |
| `r` | `message.reply` | reply |
| `e` | `message.edit` | edit your own text message (the composer must be empty) |
| `dd` | `message.delete` | delete your own message, after `y` |
| `yy` | `message.yank` | copy the message's text |
| `+` | `message.react` | react with the emoji picker; an emoji you already chose removes your reaction |
| `gx` | `message.open` | open the link, or download and open the attachment |
| `gd` | `message.goto_reply` | go to the message this one replies to |
| `R` | `message.resend` | send a message marked `failed` again |

A message that failed for a passing reason, such as a lost connection, also
goes out by itself once the sync is back; `R` is for the others.

### `composer.insert`, `composer.normal` and `composer.completion`

| Table | Key | Action | |
|---|---|---|---|
| `composer.insert` | `<CR>` | `composer.send` | send |
| `composer.insert` | `<C-j>` | `composer.newline` | a line break (many terminals send Shift+Enter as `<CR>`) |
| `composer.insert`, `composer.normal` | `<C-x><C-e>` | `composer.external_editor` | edit in `$VISUAL` or `$EDITOR` |
| `composer.normal` | `<Esc>` | `focus.timeline` | back to the timeline; the text stays |
| `composer.normal` | `<C-c>` | `composer.cancel` | end a reply (the text stays) or an edit (the text goes) |
| `composer.completion` | `<C-n>` / `<C-p>` | `completion.next` / `completion.prev` | select a mention |
| `composer.completion` | `<C-y>`, `<Tab>`, `<CR>` | `completion.accept` | insert it |
| `composer.completion` | `<C-e>` | `completion.cancel` | close the mention menu |

`<Esc>` in insert mode is not bound: the editor goes to normal mode, and a
second `<Esc>` returns to the timeline.

### `picker`, `command_line` and `message_view`

| Table | Key | Action |
|---|---|---|
| `picker` | `<C-n>` / `<C-p>` | `picker.next` / `picker.prev` |
| `picker` | `<CR>` / `<Esc>` | `picker.accept` / `picker.cancel` |
| `command_line` | `<CR>` / `<Esc>` / `<Tab>` | `command_line.execute` / `command_line.cancel` / `command_line.complete` |
| `message_view` | `q`, `<Esc>` | `view.close` |

### Actions without a default key

These actions have no default binding; bind them in the configuration, or
use the commands below:

| Action | Command |
|---|---|
| `room.open` with `filter`, `pick` or `query`, for example "open the next unread room" | `:open <room>` |
| `room.mark_read` | |
| `room.mute`, `room.unmute` | `:mute`, `:unmute` |
| `file.upload` | `:upload <path>` |
| `account.recover` | `:recover` |
| `app.quit` | `:q` |

`room.mark_read` marks the room under the cursor (in the room list) or the
open room as read. To bind it to `<leader>x`, together with opening the
next unread room:

```toml
[account]
user_id = "@alice:example.org"

[keymap.normal]
"<leader>x" = ["room.mark_read", { action = "room.open", filter = "unread", pick = "next" }]
```

Or only mark the room as read:

```toml
[account]
user_id = "@alice:example.org"

[keymap.normal]
"<leader>x" = "room.mark_read"
```

## Commands

`:` opens the command line. Everything after the command name is one
argument, so names and paths with spaces need no quotes. `<Tab>` completes
command names, and room names after `:open`.

| Command | What it does |
|---|---|
| `:open <room>` | Opens the room whose name matches `<room>` best (fuzzy). |
| `:mute` | Mutes the room: the room under the cursor in the room list, otherwise the open room. |
| `:unmute` | Unmutes it: its own notification setting is removed, so the account's default applies. A room that is not muted is left alone. |
| `:upload <path>` | Sends the file to the open room as an image, video, audio or file message, by the type its name suggests. `~/` is your home directory; a relative path is relative to where mxt was started. Anything that is not a regular file, and files larger than the homeserver's upload limit, are refused. |
| `:recover` | Not available yet: says "account.recover is not available yet" (see [First start](../README.md#first-start)). |
| `:q` | Quits. |

`:mute`, `:unmute`, `:recover` and `:q` take no argument, and `:open` and
`:upload` without one show their usage.

## The editors

The composer and the message view are [edtui](https://github.com/preiter93/edtui)
editors in vim mode.

The composer:

- `i` or `a` in the timeline moves to the composer in insert mode, after the
  character under the cursor. `<Esc>` goes to normal mode; `<Esc>` again goes
  back to the timeline. Each room keeps its own text, reply and mode.
- The text is Markdown. `<CR>` in insert mode sends it, `<C-j>` inserts a
  line break, and `<C-x><C-e>` edits it in `$VISUAL` or `$EDITOR`.
- `@` at the start of the text or after whitespace opens the mention menu with
  the room's members (at most 50, the people who wrote the loaded messages
  first); typing narrows it. The inserted `@name` is plain text: delete or
  change it, and it is no longer a mention.
- `r` and `e` in the timeline label the composer `Reply to <name>` or `Edit`.
  `<C-c>` in normal mode ends the reply or the edit.

The message view (`<CR>` on a message) holds the message's text in an
editor, so motions, `/` search, visual selection and yanks work on messages
taller than the screen. `i` and other edits work too, but are discarded when
the view closes. `q` and `<Esc>` close it in normal and visual mode.

Yanks (`yy` in the timeline, `y` in the editors) go to one register that
every editor shares, and to the terminal's clipboard
([Terminal setup](terminal.md#clipboard)). `p` pastes from that register, not
from the system clipboard; the terminal's own paste goes in as text, line
breaks included, and runs no keys.

### Differences from vim

- The editors have no count prefixes. `:` in the composer's normal mode
  opens mxt's command line.
- `f`, `F`, `t` and `T` with a character, and `;` and `,` repeating them,
  work on the cursor's line, by grapheme cluster (an emoji with a skin tone
  is one character). After `t` and `T`, `;` moves on to the next match
  instead of staying before the one it stopped at. `r` and a character
  replaces the character under the cursor in normal mode, as one undo step.
  `h`, `l`, `<Left>` and `<Right>` step over whole grapheme clusters.
- The key after `f`, `F`, `t`, `T` or `r` goes to the editor without the
  keymap: `f:` finds a colon, and `<Esc>` cancels.
- `df`, `dt`, `cf` and `ct` work, and `;` and `,` repeat their find. Other
  operators with these motions (`dF`, `cT`, `yf`, `d;`) do nothing, and the
  character after `dF` or `yf` is dropped, as is the one after `r` in visual
  mode.
- `w`, `e`, `b`, `dw`, `cw` and `iw` group characters as vim does: kanji,
  hiragana and katakana are separate words, full-width punctuation is
  punctuation, and the ideographic space is a space. Emoji count as
  punctuation, where vim gives them a class of their own.
- Not handled yet: `x`, `r`, `a`, the arrow keys in insert mode and visual
  selections work on single characters inside a grapheme cluster, so they
  can split an emoji; `e` stops on the last character of a cluster (the
  variation selector of `❤️`); `df` and `dt` can split a cluster (`df👍`
  leaves the skin tone); `df<Tab>` cancels, where `f<Tab>` finds a tab;
  operators with `F`, `T`, `;` and `,`; a linewise `p` on the last line
  pastes above it; `.` does not repeat `r`; `r` in visual mode.

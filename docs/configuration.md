# Configuration

mxt reads one TOML file when it starts:
`$XDG_CONFIG_HOME/mxt/config.toml`, or `~/.config/mxt/config.toml` when
`XDG_CONFIG_HOME` is not set, empty or a relative path (on macOS too).
`mxt --config PATH` reads another file. Changes take effect at the next start.

Only `account.user_id` is required. This is the smallest file, which mxt
prints when the file does not exist:

```toml
[account]
user_id = "@alice:example.org"
```

Every setting with its default where there is one, plus example keymap
bindings (they change the default keymap; see [Keymap](#keymap)):

```toml
[account]
user_id = "@alice:example.org"
# homeserver = "https://matrix.example.org"  # omitted: discovered via .well-known

[ui]
leader = "<Space>"
timeoutlen = 1000                                 # milliseconds
# sort: comma-separated keys from "mentions", "notify", "unread", "recent", "read", "name"
# width: 12 to 80; network: "inline" | "below"
room_list = { visible = true, sort = "recent", width = 30, network = "inline" }
time_format = "%H:%M"                             # strftime, in the local time zone
image_preview = "auto"                            # "auto" | "off" | "iterm2" | "sixel" | "kitty" | "halfblocks"
timeline = { images = "off", image_rows = 16 }    # images: "off" | "on"; image_rows: 1 to 50
read_receipts = "public"                          # "public" | "private"

[files]
download_dir = "~/Downloads"
opener = ["xdg-open"]           # ["open"] on macOS, for example ["wslview"] on WSL

[keymap.normal]                 # outside text input, in every pane
"]n" = { action = "room.open", filter = "notify", pick = "next" }
"<leader>x" = ["room.mark_read", { action = "room.open", filter = "unread", pick = "next" }]

[keymap.timeline]               # only while the timeline has focus
"dd" = "nop"                    # remove a default binding
"D" = "message.delete"

[keymap.composer.insert]        # checked before keys reach the editor
"<C-s>" = "composer.send"
```

## Errors and warnings

mxt checks the whole file before the terminal UI starts. A mistake stops the
start with the file, the line and the column, the setting, and the reason,
and mxt exits with a non-zero status. For example, with a room list that is
too narrow on line 5:

```toml
[account]
user_id = "@alice:example.org"

[ui]
room_list = { width = 5 }
```

```text
mxt: /home/alice/.config/mxt/config.toml:5:23: ui.room_list.width: must be from 12 to 80
```

These are errors:

- a TOML syntax error;
- a value of the wrong type, such as `timeoutlen = "1s"`, or `filter = 3`
  or `index = 0` in a binding;
- an `account.user_id` that is missing or not `@localpart:server`, and an
  `account.homeserver` that is not an `https://` or `http://` URL;
- a `ui.room_list.width` outside 12 to 80, a `ui.timeline.image_rows`
  outside 1 to 50, and an empty `ui.time_format`;
- a `files.download_dir` that is neither absolute nor starts with `~/`, and a
  `files.opener` without a program;
- malformed key notation, such as `"<C-x"`, `"<C-x<CR>"`, `"<>"`, `"<S-1>"`
  or `"<C-leader>"`;
- `pick` together with `query`, `emoji` together with `index`,
  `file.upload` without `path`, `room.join` without `target`, an empty
  `query`, `path`, `emoji` or `target`, a binding table without `action`,
  and an empty array as a binding;
- the same key sequence written twice in one table in different spellings,
  such as `"<leader>x"` and `"<Space>x"`, or `"<C-a>"` and `"<C-A>"`; for
  `"<C-i>"` and `"<Tab>"`, see below.

A name that this version of mxt does not know is a warning instead: one
configuration is often shared between machines that run different versions,
and a setting written for a newer mxt must not stop an older one. These are
warnings:

- a key or a table that the tables below do not list, such as `ui.font`,
  `[notifications]` or `[keymap.composer.foo]`; what is inside it is not
  checked;
- a value that a setting with fixed values does not take, such as
  `image_preview = "wezterm"`, and a key that `ui.room_list.sort` does not
  take, such as `"size"` in `sort = "mentions, size"`;
- a `ui.time_format` that mxt cannot read, since a newer version may read
  more directives;
- in a binding: an unknown action, an argument the action does not take,
  whatever its value, a value that `filter`, `pick` or `mode` does not take,
  an unknown emoji, and a key name or modifier that mxt does not know, such
  as `<F13>` or `<D-s>`; such a key name in `ui.leader` too.

The setting or binding is then ignored, and its default applies: a setting
has the default in the tables below, and the default binding of the same
keys in the same table stays, if there is one. An array with one unknown
action is ignored whole. `ui.read_receipts` is the one exception: an unknown
value sends private receipts, not the default public ones. While an unknown
`ui.leader` falls back to `<Space>`, two spellings in one table that only
the fallback makes the same keys, such as `"<leader>x"` and `"<Space>x"`,
are not an error: the later of the two in the file is ignored with a warning.

`ui.room_list.sort` is a list, so an unknown key ignores only that key, and
the other keys apply in their order; when every key is unknown, `"recent"`
applies. A list that mxt cannot use is a warning too, and `"recent"`
applies: a list that names no key (`sort = ""`), a list with an empty key
(two commas in a row, or one at the start or the end), and a list that
names a key twice. The warnings read `ui.room_list.sort: names no sort key
(ignored)`, `ui.room_list.sort: empty sort key in "mentions,,read"
(ignored)` and `ui.room_list.sort: "unread" is listed twice (ignored)`.
mxt 0.1.3, which knows only `"recent"` and `"name"`, reads a list or a new
key as an unknown value: a warning, and `"recent"` applies. mxt 0.1.4 does
not know `"notify"`: it skips that key with a warning and applies the
others, so it sorts `"mentions, notify, unread, read"` as
`"mentions, unread, read"`. So a configuration shared with machines that
run 0.1.3 or 0.1.4 can carry a list and the `"notify"` key.

Up to mxt 0.1.2, `<C-i>` and `<Tab>` were two keys, and `<C-i>`, which
terminals send as `<Tab>`, never arrived; now `<C-i>` is another name for
`<Tab>` (see [Key notation](#key-notation)). A table that binds both, such
as `"<Tab>"` and `"<C-i>"` or `"<C-w><Tab>"` and `"<C-w><C-i>"`, still
loads: the binding written with `<C-i>` is ignored with a warning,
whichever comes first, since only the `<Tab>` one ever ran. For example,
with `"<Tab>"` on line 4 and `"<C-i>"` on line 5:

```text
/home/alice/.config/mxt/config.toml:5:1: keymap.picker: "<C-i>" is the same key sequence as "<Tab>" on line 4, since <C-i> is another name for <Tab> (ignored)
```

Spellings that were one key sequence before stay an error, such as
`"<C-i>"` and `"<C-I>"`.

A warning has the form of an error and ends in `(ignored)`. For example, with
a misspelled action on line 5:

```toml
[account]
user_id = "@alice:example.org"

[keymap.timeline]
"x" = "message.dlete"
```

```text
/home/alice/.config/mxt/config.toml:5:7: keymap.timeline.x: unknown action "message.dlete" (ignored)
```

mxt starts anyway, and the status line says `Ignored 1 setting in the
configuration; see :messages` (or how many) in the error color. `:messages`
lists each warning, and the log has them too (see
[Files](../README.md#files)). Nothing is silently ignored: a typo is an
error, or a warning that the status line points to at every start.

A misspelled required key is both: mxt stops because the key is missing,
and prints the warnings it found before the error, which name the
misspelling. For example, with `[acount]` on line 1:

```text
mxt: /home/alice/.config/mxt/config.toml:1:1: missing field `account`
/home/alice/.config/mxt/config.toml:1:2: acount: unknown table (ignored)
```

Within one binding the first problem decides, in this order: its keys, the
same keys spelled twice, then each action, its name, the names of its
arguments, conflicts between them, and their values. So
`{ action = "room.open", sort = "name", pick = "next", query = "x" }` is a
warning about `sort`, and
`{ action = "room.open", pick = "next", query = "x", filter = "dm" }` an
error about `pick` with `query`.

## `[account]`

| Key | Default | Values | Meaning |
|---|---|---|---|
| `user_id` | required | a Matrix user ID, `@localpart:server` | The account to log in to. A saved session of another account stops the start (see [Files](../README.md#files)). |
| `homeserver` | discovered | an `https://` or `http://` URL | The homeserver. When it is not set, mxt asks the server named in `user_id` (`.well-known`). Once a session is saved, the homeserver of the session is used, so a change takes effect only at the next login. |

## `[ui]`

| Key | Default | Values | Meaning |
|---|---|---|---|
| `leader` | `"<Space>"` | [key notation](#key-notation), without `<leader>` | What `<leader>` stands for in the default keymap and in your bindings. A key name that mxt does not know, such as `"<F13>"`, is ignored with a warning, and `<Space>` applies. |
| `timeoutlen` | `1000` | milliseconds, a non-negative integer | How long a key sequence that starts a longer binding waits for the next key, like nvim's `timeoutlen`. |
| `room_list` | see below | a table | The room list. |
| `timeline` | see below | a table | Images in the timeline. |
| `time_format` | `"%H:%M"` | a [strftime format](https://docs.rs/jiff/latest/jiff/fmt/strtime/index.html), not empty | The time in each message header, in the local time zone. A format that mxt cannot read is ignored with a warning, and `"%H:%M"` applies; an empty one is an error. |
| `image_preview` | `"auto"` | `"auto"`, `"off"`, `"iterm2"`, `"sixel"`, `"kitty"`, `"halfblocks"` | `"auto"` shows images in the message view, and with `timeline.images = "on"` in the timeline, in the graphics protocol mxt chooses for the terminal (see [Terminal setup](terminal.md#images)). `"iterm2"`, `"sixel"`, `"kitty"` and `"halfblocks"` use that protocol instead, for a terminal where the choice is wrong. `"off"` shows only the file name and size in the view and no previews in the timeline, and mxt never asks the terminal for its graphics protocol. |
| `read_receipts` | `"public"` | `"public"`, `"private"` | `"public"` sends `m.read` receipts, which other users see. `"private"` sends `m.read.private`, which only your own devices see. An unknown value, such as one a newer mxt knows, sends private receipts, not the default public ones, with a warning that says so: receipts go out as soon as a room opens, before you may have seen the warning. |

`room_list` is a table; write it inline as in the example, or as a section:

```toml
[account]
user_id = "@alice:example.org"

[ui.room_list]
sort = "name"
network = "below"
```

| Key | Default | Values | Meaning |
|---|---|---|---|
| `visible` | `true` | `true`, `false` | Whether the room list is shown at startup. `<leader>e` (`sidebar.toggle`) shows and hides it. |
| `sort` | `"recent"` | a comma-separated list of the [sort keys](#sort-keys), such as `"mentions, unread, read"` | The order of the rooms. The keys apply in order, each deciding only between the rooms that the keys before it leave tied, and rooms that every key leaves tied go by name. Spaces around the commas are optional, and keys are lowercase. A list of one key sorts as before, so `"recent"` and `"name"` keep their meaning. |
| `width` | `30` | an integer from 12 to 80 | The room list's width in columns, its border included. A terminal narrower than 90 columns gives it a third of its width instead, and it never takes more than half. |
| `network` | `"inline"` | `"inline"`, `"below"` | Where a bridged room's network name goes. `"inline"`: one row, `Discord / Alice`. `"below"`: the room name on the first row with the badge, and the network name dim on a second row. A room without a network name keeps one row. |

### Sort keys

| Key | First |
|---|---|
| `"mentions"` | rooms with unread mentions of you: whether a room has any, not how many |
| `"notify"` | rooms that notify you: unread mentions, bold counts and the manual unread mark, the rooms that `filter = "notify"` takes; not the rooms whose only news is a dim count, such as muted rooms and rooms set to mentions and keywords only |
| `"unread"` | rooms with unread messages, the rooms that `filter = "unread"` takes: any badge |
| `"recent"` | the latest activity, newest first; rooms without any last |
| `"read"` | the latest news for you, newest first: a room with unread messages (any badge, as for `"unread"`) by the time of its newest message, any other room by your own latest read receipt in it outside threads, sent from any device; the room you have open counts as read up to its newest message; rooms you never read and with nothing unread last |
| `"name"` | by the displayed name (`network / room`), ignoring case, which groups rooms by network |

`sort = "mentions, unread, read"` lists the rooms with unread mentions
first, then the other unread rooms, then the rest. In the two unread groups
the room with the newest message comes first, so a room that gets a new
message goes to the top of its group, and the read rooms are in the order
you last read them. `"read"` alone orders the rooms by when each last had
news for you or you last read it.

The room you have open sorts as read. With `"unread"`, `"notify"` or
`"mentions"` it does not move into or within the unread rooms while it is
open, though its badge counts new messages. With `"read"` it counts as read
up to its newest message, so a room with something new takes its place
among the read rooms as you open it. Once you open another room, it sorts
as any other room again. When you read a room with something new (its read
receipt goes out, or `room.mark_read`), it goes to the top of the read rooms
at once and stays there when the homeserver sends the receipt back. Opening
a room with nothing new leaves it in place, since homeservers keep a read
receipt only when it moves forward, when you last read the room in mxt or
in a client that sends the same kind of read receipt; a room you last read
in a client that marks only the main timeline read, as Element does, or
with the other kind of receipt (`read_receipts`) can go to the top of the
read rooms once: when your receipt of mxt's kind in it is older, the
homeserver keeps mxt's receipt and sends it back. A room whose only
news is an edit of an older message goes to the time of the edit while
open, and back afterwards.

`sort = "mentions, notify, unread, read"` puts the rooms that notify you
after the rooms with mentions and before the rooms whose only news is a dim
count, such as busy bridged channels that are muted or set to mentions and
keywords only, each group newest message first. `"unread"`,
`filter = "unread"` and the default `]u` and `[u` still count dim counts;
with these bindings, `]u` and `[u` skip those rooms too:

```toml
[account]
user_id = "@alice:example.org"

[ui]
room_list = { sort = "mentions, notify, unread, read" }

[keymap.normal]
"]u" = { action = "room.open", filter = "notify" }
"[u" = { action = "room.open", filter = "notify", pick = "prev" }
```

### `[ui.timeline]`

`timeline` is a table too, inline as in the example or as a section:

```toml
[account]
user_id = "@alice:example.org"

[ui.timeline]
images = "on"
image_rows = 12
```

| Key | Default | Values | Meaning |
|---|---|---|---|
| `images` | `"off"` | `"off"`, `"on"` | `"on"` shows image messages and stickers with a preview under their `[image: …]` or `[sticker: …]` line, in the graphics protocol of the message view (see [Terminal setup](terminal.md#previews-in-the-timeline)). `ui.image_preview = "off"` turns previews off too. |
| `image_rows` | `16` | an integer from 1 to 50 | The most rows a preview takes. A preview keeps the image's proportions within that many rows and the timeline's width, and is never larger than the image. |

mxt 0.1.4 does not know `ui.timeline`: it ignores the table with a
warning, so a configuration shared with machines that run 0.1.4 can carry
it. Up to mxt 0.1.5, `image_rows` was 8 by default; `image_rows = 8` keeps
that size.

## `[files]`

| Key | Default | Values | Meaning |
|---|---|---|---|
| `download_dir` | `"~/Downloads"` | an absolute path, or one that starts with `~/` | Where attachments are saved before they are opened. A leading `~` is your home directory. The directory is created when a download needs it. A file never replaces another: the second `photo.png` is saved as `photo (1).png`. |
| `opener` | `["xdg-open"]`, `["open"]` on macOS | an array: a program and its arguments | What opens downloaded files and links. mxt appends the path or URL as one more argument, runs the program without a shell, does not wait for it, and discards its output. On WSL, `["wslview"]` opens them in Windows. Over SSH it runs on the machine mxt runs on. |

## Keymap

`[keymap.<table>]` sections bind key sequences in one table each. Your
bindings are merged over the [default keymap](keys.md#default-keymap) key by
key: a binding replaces the default binding of the same keys in the same
table, and every other default stays.

| Table | Used when | If the key is not bound there |
|---|---|---|
| `normal` | (fallback only) | depends on the table that falls back to it |
| `room_list` | the room list has focus | look in `normal`, then ignore the key |
| `timeline` | the timeline has focus | look in `normal`, then ignore the key |
| `thread` | the timeline has focus and shows a thread (see [Threads](keys.md#threads)) | look in `timeline`, then `normal`, then ignore the key |
| `composer.normal`, `composer.visual` | the composer has focus and its editor is in normal or visual mode | look in `normal`, then pass the key to the editor |
| `composer.insert` | the composer's editor is in insert mode | pass the key to the editor |
| `composer.completion` | the mention menu is open in insert mode | look in `composer.insert`, then pass the key to the editor |
| `message_view` | the message view (also `:help`, `:map` and `:messages`) is open and its editor is in normal or visual mode | pass the key to the editor |
| `picker` | a picker (rooms, emoji or links) is open | edit the query with the key (see [Line editing](keys.md#line-editing)) |
| `command_line` | the `:` command line or a `/` search line is open | edit the line with the key (see [Line editing](keys.md#line-editing)) |

`normal` holds bindings that make sense everywhere outside text input. The
movement keys are in `room_list` and `timeline`, so `j` and `k` still reach
the composer's editor. Only `room_list`, `timeline` and `thread` take a
count prefix (`5j`).

The default `t` (`thread.open`) is in `timeline`, which is checked before
`normal`: a binding of `t` in `[keymap.normal]` does not apply while the
timeline has focus unless you write it in `[keymap.timeline]` too. The
default `q` and `<Esc>` (`thread.close`) are in `thread`, so the timeline
itself binds neither, and a binding of them in `[keymap.normal]` applies
there.

The default `<C-o>` and `<Tab>` (`jump.back` and `jump.forward`) are in
`normal`, so they work in the room list, the timeline and the composer's
normal and visual modes. The composer's insert mode, the mention menu,
pickers, the command line and the message view do not look in `normal`
and keep their own `<Tab>` and `<C-o>` (see
[The jump list](keys.md#the-jump-list)).

A table that is checked first wins: with `"]"` in `timeline` and the default
`"]u"` in `normal`, `]` in the timeline waits for the next key, as in nvim,
and runs the `timeline` binding when the next key is not `u` or when
`timeoutlen` passes. Some keys never reach the keymap: in the editors' search
mode, after a key that waits for its character (`f`, `F`, `t`, `T`, `r`),
after an operator (`d`, `c`, `y`), after a text object's `i` or `a` and after
`g`, as in nvim, after `"` and in the command after `"_`, and in the message
view's insert mode, every key goes to the editor (see
[Keys](keys.md#the-editors)). A binding that starts with one of these keys,
such as `"gt"` in `normal`, still runs: the keymap gets that key before the
editor does.

### Binding values

- A string names an action without arguments: `"picker.rooms"`.
- A table names an action with arguments:
  `{ action = "room.open", filter = "unread", pick = "next" }`.
- An array runs several actions in order, each on the state the previous one
  left: `["room.mark_read", { action = "room.open", filter = "unread", pick = "next" }]`.
- `"nop"` does nothing, like nvim's `<Nop>`. It hides the same keys in the
  tables after it, which is how you remove a default binding.

A binding with arguments can also be a section of its own:

```toml
[account]
user_id = "@alice:example.org"

[keymap.normal."]n"]
action = "room.open"
filter = "notify"
pick = "next"
```

### Key notation

Keys use nvim's notation. Key sequences are TOML strings, so quote them:
`"gg"`, `"<C-x><C-e>"`, `"<leader>m"`.

- A character stands for itself: `j`, `G`, `+`, `/`, `あ`. An uppercase
  letter is the letter typed with shift.
- `<...>` names a key, optionally with modifiers: `C-` (ctrl), `A-` or `M-`
  (alt) and `S-` (shift), in any order and any case: `<C-w>`, `<A-x>`,
  `<S-Tab>`, `<C-A-x>`.
- Key names, in any case: `CR` (also `Enter`, `Return`), `Esc`, `Tab`, `BS`
  (also `Backspace`), `Del` (also `Delete`), `Insert` (also `Ins`), `Up`,
  `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `Space`,
  `F1` to `F12`, and `lt`, `Bslash` and `Bar` for `<`, `\` and `|`.
- `<leader>` is `ui.leader`; it takes no modifiers.
- A `<` without its `>` is an error, also when the `>` of a later key
  follows (`"<C-x<CR>"`), and so is `<>`; write `<lt>` for `<`. A key name
  or a modifier that mxt does not know, such as `<F13>` or `<D-s>`, ignores
  the binding with a warning (see [Errors and warnings](#errors-and-warnings)).
- With ctrl, letters have no case: `<C-a>` and `<C-A>` are the same key.
  `S-` on a character must give an uppercase letter (`<S-a>` is `A`), so
  write `!` instead of `<S-1>`.

`<C-i>` is another name for `<Tab>`, and `<C-A-i>` for `<A-Tab>`, as in
nvim: without the kitty keyboard protocol, which mxt does not turn on,
terminals send the same key for both, and `:map` shows `<Tab>`. Terminals
send other keys alike too: `<C-m>` arrives as `<CR>`, `<C-[>` as `<Esc>`,
and many terminals send Shift+Enter as `<CR>`. Bind the key that arrives.

### Examples

`]u` opens the topmost unread room other than the open one by default, and
`[u` the previous unread room. Open the next room that notifies you after
the open one with `]n`, and the previous one with `[n`:

```toml
[account]
user_id = "@alice:example.org"

[keymap.normal]
"]n" = { action = "room.open", filter = "notify", pick = "next" }
"[n" = { action = "room.open", filter = "notify", pick = "prev" }
```

Mark the current room as read and open the next unread room with
`<leader>x`. `room.mark_read` has no default binding. It acts on the room
under the cursor while the room list has focus, and on the open room
otherwise, and it counts the room as read at once, so the `room.open` after
it skips that room:

```toml
[account]
user_id = "@alice:example.org"

[keymap.normal]
"<leader>x" = ["room.mark_read", { action = "room.open", filter = "unread", pick = "next" }]
```

Use `,` as the leader, open a room by name with `<leader>g`, and quit with
`<leader>q`:

```toml
[account]
user_id = "@alice:example.org"

[ui]
leader = ","

[keymap.normal]
"<leader>g" = { action = "room.open", query = "general" }
"<leader>q" = "app.quit"
```

Delete messages with `D` instead of `dd`, send with `<C-s>` as well as
`<CR>`, and keep the editor's mode when the composer gets focus with `i`:

```toml
[account]
user_id = "@alice:example.org"

[keymap.timeline]
"dd" = "nop"
"D" = "message.delete"
"i" = { action = "composer.focus", mode = "keep" }

[keymap.composer.insert]
"<C-s>" = "composer.send"
```

### Actions

Actions are named `<target>.<verb>`. Only the actions marked so take
arguments; any other argument ignores the binding with a warning. Actions on
"the message" use the message under the timeline cursor.

| Action | Arguments | What it does |
|---|---|---|
| `room.open` | `filter`, and `pick` or `query` (below) | Opens the room the arguments choose, or says "No matching room". |
| `room.open_selected` | | Opens the room under the room list cursor. |
| `room.mark_read` | | Marks the room as read: the room under the cursor while the room list has focus, otherwise the open room. Sends a read receipt for the room's latest message, moves the fully-read marker there, and clears the manual unread mark. |
| `room.mute` | | Mutes the room (the same room as `room.mark_read`). |
| `room.unmute` | | Removes the room's own notification setting, so the account's default applies again. On a room that is not muted it does nothing and says so, so a "mentions only" setting made in another client stays. |
| `room.join` | `target`, required | Joins a room or opens a direct message as `:join <target>` does (see [Keys](keys.md#joining-rooms-and-direct-messages)): `target` is a room alias, a room ID, a user ID, a matrix.to link or the name of a pending invite, read when the key runs, for example `{ action = "room.join", target = "#mxt:example.org" }`. An empty `target` is an error. |
| `message.reply` | | Starts a reply to the message in the composer. Not for a message that is still being sent, and not while an edit is open in the composer. |
| `message.edit` | | Loads your own text message into the composer to edit it. The composer must be empty. |
| `message.delete` | | Deletes your own message after you confirm with `y`. |
| `message.yank` | | Copies the message's text to the clipboard (see [Terminal setup](terminal.md#clipboard)). |
| `message.react` | `emoji` or `index` | Without arguments, opens the emoji picker, which lists the reactions shown under the message first: choosing one of them toggles its own key, removing yours if you are among its senders or joining it otherwise; choosing a typed emoji or any other entry toggles it as `emoji` does, removing your reaction with that emoji, whatever its key, or adding it. With `emoji`, an emoji (`"👍"`) or a shortcode with colons on both sides or none (`"thumbsup"`, `":+1:"`), toggles that reaction: adds it, or removes yours. An empty `emoji` is an error; an unknown one ignores the binding with a warning, since a newer mxt may know it. With `index = N`, a positive integer, toggles the N-th reaction in the order they are shown under the message (under `👍 2  ❤️ 1`, `index = 1` is 👍), as the emoji picker's N-th entry does: joins it with its key, or removes yours if you are among its senders, so a second press removes it. If yours is a plain ❤ (without the variation selector) in `❤ 1  ❤️ 1`, `index = 2` adds a ❤️ rather than removing your ❤, which `emoji = "heart"` would do. `g1` … `g9` in `timeline` are the defaults for `index = 1` … `9`, written as `"g1" = { action = "message.react", index = 1 }` and so on. Without an N-th reaction, it says so and sends nothing. `emoji` and `index` together, and an `index` that is not a positive integer, are errors. With no open room, on an empty timeline or on a deleted message, it says so and does nothing. |
| `message.open` | | Downloads an attachment to `files.download_dir` and opens it with `files.opener`, or opens the message's web link with `files.opener`, or offers a link picker when there are several. Only `http://` and `https://` links. |
| `message.goto_reply` | | Moves the cursor to the message this one replies to, if the shown timeline holds it. In a thread, it goes to a message of the timeline, closing the thread; a message that is a reply in another thread opens that thread with the cursor on it (see [Threads](keys.md#threads)). Otherwise it says the message is not loaded. |
| `message.resend` | | Sends a failed message again. |
| `message.view` | | Opens the message in a full-screen view: text in an editor where vim motions, search, visual selection and yanks work (changes are discarded), or the image of an image message or a sticker. |
| `composer.focus` | `mode` | Moves the focus to the composer of the shown timeline, the open room's or its thread's. `mode = "insert"` (default) starts insert mode after the character under the cursor; `mode = "keep"` keeps the editor's last mode. |
| `composer.send` | | Sends the composer's text as Markdown. A blank text does nothing. |
| `composer.newline` | | Inserts a line break. |
| `composer.cancel` | | Ends a reply (the text stays) or an edit (the composer is cleared). |
| `composer.leave` | | Moves the focus to the timeline. When the composer is empty, a reply or an edit ends first, as with `composer.cancel`; any text, even a space, stays with its reply or edit. `focus.timeline` leaves the reply or edit as it is. |
| `composer.external_editor` | | Edits the composer's text in `$VISUAL` or `$EDITOR`. |
| `composer.normal` | | Puts the composer's editor in normal mode, as `<Esc>` does in insert mode (the cursor moves back one character and `.` repeats the insert); a visual selection is dropped and a search stopped. In normal mode it does nothing. After `composer.send`, as in `"<CR>" = ["composer.send", "composer.normal"]`, the editor is in normal mode after sending, also when a blank text sends nothing. |
| `composer.delete_word_before` | | In insert mode, deletes the word before the cursor and the blanks after it, split into words as the command line's `<C-w>` splits them (see [Keys](keys.md#line-editing)); at the start of a line it deletes the line break, joining the line to the one above. `u` in normal mode brings the deleted text back, and the register is not changed. The action does nothing in normal and visual mode. |
| `composer.clear` | | Empties the composer as one change: `u` in normal mode brings the text back, and the register is not changed. A reply or an edit stays (`composer.cancel` ends it). The editor keeps its mode, except that visual mode becomes normal mode. It has no default key. |
| `completion.next`, `completion.prev` | | Selects the next or previous member in the mention menu. |
| `completion.accept` | | Inserts the selected mention. |
| `completion.cancel` | | Closes the mention menu. |
| `picker.rooms` | `filter` | Opens the fuzzy room picker over the rooms that pass `filter`. |
| `picker.next`, `picker.prev` | | Selects the next or previous item. |
| `picker.accept` | | Takes the selected item and closes the picker. |
| `picker.cancel` | | Closes the picker. |
| `command_line.open` | | Opens the `:` command line. |
| `command_line.execute` | | Runs the command, or searches with a `/` line. |
| `command_line.cancel` | | Closes the line. |
| `command_line.complete` | | Completes command names, room names after `:open`, the names of pending invites after `:join`, and emoji shortcodes after `:react`; pressing it again cycles through the candidates. |
| `command_line.history_prev` | | Puts the next older line of the history of the command line or the `/` search line that starts with the text that was before the cursor at the first press, with the cursor at its end; with none left it does nothing (see [History](keys.md#history)). |
| `command_line.history_next` | | While browsing the history, goes to the next newer such line, and past the newest brings back the typed text and cursor. Otherwise it does nothing. |
| `cursor.down`, `cursor.up` | | Moves the cursor of the focused list by one room or message, or by the count. |
| `cursor.first`, `cursor.last` | | Moves it to the first or last room or message. |
| `cursor.half_page_down`, `cursor.half_page_up` | | Moves it by half of the visible rooms or messages, times the count. |
| `search.start` | | Opens a `/` line that searches the focused list. |
| `search.next`, `search.prev` | | Repeats the last search forwards or backwards. |
| `search.clear_highlight` | | Hides the search highlight until the next search, as `:noh` and `:nohlsearch` do. After `/`, `n` or `N`, the matches of the pattern in the timeline's messages are shown in reverse video, bold in the message under the cursor, until this action or until another room opens (see [Keys](keys.md#room_list-and-timeline)). It has no default key. |
| `jump.back` | | Goes back to where the timeline cursor was before the last jump (opening another room, opening or closing a thread, `gg`, `G`, a search or `gd` in the timeline), opening its room and its thread and loading older messages when needed, as nvim's `<C-o>` does; see [The jump list](keys.md#the-jump-list). From the newest place it first notes where you are, so `jump.forward` comes back to it. A place in a room you left or with a message that is gone is skipped. |
| `jump.forward` | | Goes forward again in the jump list, as nvim's `<C-i>` does. |
| `thread.open` | | Shows the thread of the message under the timeline cursor in place of the timeline, or starts a thread on a message without replies; in a thread it only focuses the timeline. Not for a message that is still being sent, nor for a deleted message without replies. See [Threads](keys.md#threads), also for its read receipts. |
| `thread.close` | | Shows the timeline again in place of the thread, with the cursor on the thread's root. Without a thread it does nothing. |
| `view.close` | | Closes the message view, also when it shows `:help`, `:map` or `:messages`. |
| `view.leave` | | In the message view's visual mode, goes back to normal mode with the cursor where it is, as `<Esc>` does in vim; otherwise closes the view, as `view.close` does. The default `<Esc>` in `message_view`, where `q` stays `view.close`. |
| `help.commands` | | Shows the commands with a one-line description each in the full-screen view, as `:help` does. |
| `help.keymap` | | Shows the bindings of each keymap table after your configuration in the full-screen view, as `:map` does: `keys -> action` or `keys -> action (arguments)`, with `<leader>` written as its key. |
| `status.messages` | | Shows the status line's recent messages, oldest first, in the full-screen view, as `:messages` does. |
| `sidebar.toggle` | | Shows or hides the room list. |
| `focus.room_list` | | Moves the focus to the room list, and shows it if it is hidden. |
| `focus.timeline` | | Moves the focus to the timeline. |
| `file.upload` | `path`, required | Uploads the file at `path` to the open room, or into the thread shown in place of its timeline. `~/` is your home directory; a relative path is relative to the directory mxt was started in. |
| `account.recover` | | Asks for the recovery key in `SECRET` mode, masked; `<CR>` submits and `<Esc>` cancels. As on the first start, the key cross-signs this device and lets it fetch room keys from the key backup. |
| `app.quit` | | Quits mxt. |
| `nop` | | Does nothing. |

`room.open` and `picker.rooms` choose among the rooms that pass `filter`, in
the room list's order:

| Argument | Value | Meaning |
|---|---|---|
| `filter` | `"all"` (default) | every room in the room list |
| | `"unread"` | rooms with any badge: unread messages, notifications or mentions, or the manual unread mark |
| | `"notify"` | rooms with notifications or mentions, or the manual unread mark |
| | `"mentions"` | rooms with unread mentions |
| `pick` | `"first"` (default) | the first such room other than the open room |
| | `"next"` | the first one after the open room, wrapping around, never the open room itself; the first one when no room is open |
| | `"prev"` | the same, backwards; the last one when no room is open |
| `query` | a non-empty string | the best fuzzy match for the query among their names |

`room.open` takes `pick` or `query`, not both. `picker.rooms` takes only
`filter`.

## Environment variables

| Variable | Meaning |
|---|---|
| `XDG_CONFIG_HOME` | the configuration file's directory, see above |
| `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`, `HOME` | where mxt keeps its data, see [Files](../README.md#files) |
| `MXT_LOG` | the log filter, see [Files](../README.md#files) |
| `TMUX`, `WSL_DISTRO_NAME`, `SSH_CONNECTION`, `SSH_TTY` | how `ui.image_preview = "auto"` chooses the graphics protocol, and whether `"sixel"` goes to tmux, see [Images](terminal.md#images) |
| `VISUAL`, `EDITOR` | the external editor (`<C-x><C-e>`); a platform default when neither is set |

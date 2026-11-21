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
# sort: comma-separated keys from "mentions", "unread", "recent", "read", "name"
# width: 12 to 80; network: "inline" | "below"
room_list = { visible = true, sort = "recent", width = 30, network = "inline" }
time_format = "%H:%M"                             # strftime, in the local time zone
image_preview = "auto"                            # "auto" | "off" | "iterm2" | "sixel" | "kitty" | "halfblocks"
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
  in a binding;
- an `account.user_id` that is missing or not `@localpart:server`, and an
  `account.homeserver` that is not an `https://` or `http://` URL;
- a `ui.room_list.width` outside 12 to 80, and an empty `ui.time_format`;
- a `files.download_dir` that is neither absolute nor starts with `~/`, and a
  `files.opener` without a program;
- malformed key notation, such as `"<C-x"`, `"<C-x<CR>"`, `"<>"`, `"<S-1>"`
  or `"<C-leader>"`;
- `pick` together with `query`, `file.upload` without `path`, `room.join`
  without `target`, an empty `query`, `path`, `emoji` or `target`, a
  binding table without `action`, and an empty array as a binding;
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
key as an unknown value: a warning, and `"recent"` applies. So a
configuration shared with machines that run 0.1.3 can carry a list.

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
| `time_format` | `"%H:%M"` | a [strftime format](https://docs.rs/jiff/latest/jiff/fmt/strtime/index.html), not empty | The time in each message header, in the local time zone. A format that mxt cannot read is ignored with a warning, and `"%H:%M"` applies; an empty one is an error. |
| `image_preview` | `"auto"` | `"auto"`, `"off"`, `"iterm2"`, `"sixel"`, `"kitty"`, `"halfblocks"` | `"auto"` shows images in the message view in the graphics protocol mxt chooses for the terminal (see [Terminal setup](terminal.md#images)). `"iterm2"`, `"sixel"`, `"kitty"` and `"halfblocks"` use that protocol instead, for a terminal where the choice is wrong. `"off"` shows only the file name and size there, and mxt never asks the terminal for its graphics protocol. |
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
| `"unread"` | rooms with unread messages, the rooms that `filter = "unread"` takes: any badge |
| `"recent"` | the latest activity, newest first; rooms without any last |
| `"read"` | the rooms you read most recently, by your own latest read receipt in the room outside threads, sent from any device; rooms you never read last |
| `"name"` | by the displayed name (`network / room`), ignoring case, which groups rooms by network |

`sort = "mentions, unread, read"` lists the rooms with unread mentions
first, then the other unread rooms, then the rest, each group by when you
last read its rooms. With `"read"`, a room you open moves up when its newest
message is newer than your last read receipt in it, once the homeserver
sends the receipt back; opening a room with nothing new leaves it in place,
since homeservers keep a read receipt only when it moves forward. With
`"unread"` or `"mentions"`, a room you read moves at once.

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
| `composer.normal`, `composer.visual` | the composer has focus and its editor is in normal or visual mode | look in `normal`, then pass the key to the editor |
| `composer.insert` | the composer's editor is in insert mode | pass the key to the editor |
| `composer.completion` | the mention menu is open in insert mode | look in `composer.insert`, then pass the key to the editor |
| `message_view` | the message view (also `:help`, `:map` and `:messages`) is open and its editor is in normal or visual mode | pass the key to the editor |
| `picker` | a picker (rooms, emoji or links) is open | edit the query with the key (see [Line editing](keys.md#line-editing)) |
| `command_line` | the `:` command line or a `/` search line is open | edit the line with the key (see [Line editing](keys.md#line-editing)) |

`normal` holds bindings that make sense everywhere outside text input. The
movement keys are in `room_list` and `timeline`, so `j` and `k` still reach
the composer's editor. Only `room_list` and `timeline` take a count prefix
(`5j`).

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
`g`, as in nvim, and in the message view's insert mode, every key goes to the
editor (see [Keys](keys.md#the-editors)). A binding that starts with one of
these keys, such as `"gt"` in `normal`, still runs: the keymap gets that key
before the editor does.

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

`]u` and `[u` open the next and previous unread room by default. Open the
next room that notifies you with `]n`, and the previous one with `[n`:

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
| `message.react` | `emoji` | Without `emoji`, opens the emoji picker; choosing an emoji you already reacted with removes your reaction. With `emoji`, an emoji (`"👍"`) or a shortcode with colons on both sides or none (`"thumbsup"`, `":+1:"`), toggles that reaction: adds it, or removes yours. An empty `emoji` is an error; an unknown one ignores the binding with a warning, since a newer mxt may know it. With no open room, on an empty timeline or on a deleted message, it says so and does nothing. |
| `message.open` | | Downloads an attachment to `files.download_dir` and opens it with `files.opener`, or opens the message's web link with `files.opener`, or offers a link picker when there are several. Only `http://` and `https://` links. |
| `message.goto_reply` | | Moves the cursor to the message this one replies to, if it is loaded. |
| `message.resend` | | Sends a failed message again. |
| `message.view` | | Opens the message in a full-screen view: text in an editor where vim motions, search, visual selection and yanks work (changes are discarded), or an image. |
| `composer.focus` | `mode` | Moves the focus to the open room's composer. `mode = "insert"` (default) starts insert mode after the character under the cursor; `mode = "keep"` keeps the editor's last mode. |
| `composer.send` | | Sends the composer's text as Markdown. A blank text does nothing. |
| `composer.newline` | | Inserts a line break. |
| `composer.cancel` | | Ends a reply (the text stays) or an edit (the composer is cleared). |
| `composer.leave` | | Moves the focus to the timeline. When the composer is empty, a reply or an edit ends first, as with `composer.cancel`; any text, even a space, stays with its reply or edit. `focus.timeline` leaves the reply or edit as it is. |
| `composer.external_editor` | | Edits the composer's text in `$VISUAL` or `$EDITOR`. |
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
| `cursor.down`, `cursor.up` | | Moves the cursor of the focused list by one room or message, or by the count. |
| `cursor.first`, `cursor.last` | | Moves it to the first or last room or message. |
| `cursor.half_page_down`, `cursor.half_page_up` | | Moves it by half of the visible rooms or messages, times the count. |
| `search.start` | | Opens a `/` line that searches the focused list. |
| `search.next`, `search.prev` | | Repeats the last search forwards or backwards. |
| `jump.back` | | Goes back to where the timeline cursor was before the last jump (opening another room, `gg`, `G`, a search or `gd` in the timeline), opening its room and loading older messages when needed, as nvim's `<C-o>` does; see [The jump list](keys.md#the-jump-list). From the newest place it first notes where you are, so `jump.forward` comes back to it. A place in a room you left or with a message that is gone is skipped. |
| `jump.forward` | | Goes forward again in the jump list, as nvim's `<C-i>` does. |
| `view.close` | | Closes the message view, also when it shows `:help`, `:map` or `:messages`. |
| `help.commands` | | Shows the commands with a one-line description each in the full-screen view, as `:help` does. |
| `help.keymap` | | Shows the bindings of each keymap table after your configuration in the full-screen view, as `:map` does: `keys -> action` or `keys -> action (arguments)`, with `<leader>` written as its key. |
| `status.messages` | | Shows the status line's recent messages, oldest first, in the full-screen view, as `:messages` does. |
| `sidebar.toggle` | | Shows or hides the room list. |
| `focus.room_list` | | Moves the focus to the room list, and shows it if it is hidden. |
| `focus.timeline` | | Moves the focus to the timeline. |
| `file.upload` | `path`, required | Uploads the file at `path` to the open room. `~/` is your home directory; a relative path is relative to the directory mxt was started in. |
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
| `pick` | `"first"` (default) | the first such room |
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

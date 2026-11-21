# Keys

mxt's keys follow nvim. `<leader>` is `<Space>` unless `ui.leader` says
otherwise. Every binding below can be changed or removed, and any action can
be bound; see [Configuration](configuration.md#keymap) for the tables, the
key notation and the actions. With `ui.leader` set to `<Tab>` or `<C-o>`,
or a binding such as `"<Tab>x"`, that key alone is now also bound, to
`jump.forward` or `jump.back` (see [The jump list](#the-jump-list)): when
no other key follows within `ui.timeoutlen`, it runs that action, where
mxt 0.1.2 did nothing. The leader and the longer bindings work as before.

## The screen

- The room list is on the left; `<leader>e` hides and shows it. The open
  room's timeline is on the right, with the composer below it.
- The room list is in the order of `ui.room_list.sort`, by latest activity
  unless it says otherwise (see [Sort keys](configuration.md#sort-keys)).
  When rooms move, as messages arrive or as you read them, the cursor stays
  on its room. `]u`, `[u` and the room picker follow the list's order.
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

Counts above 99 show as `99+`. Muted rooms also show `Muted`.

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
When newer messages are below the pane, for example after a burst arrived
while the cursor was on an older message, the composer's top rule ends with
`↓ N newer`. `N` counts the messages below the pane, yours and those still
sending too, but not date dividers or the `new messages` line; above 99 it
reads `99+`. On a narrow rule it shortens to `↓ N` or is left out. It goes
once the pane shows the newest message, as moving down to it does.

## Modes

| Mode in the status line | When | Keys |
|---|---|---|
| `NORMAL` | the room list or the timeline has focus | the keymap |
| `INSERT`, `NORMAL`, `VISUAL`, `SEARCH` | the composer has focus; the mode of its editor | the keymap, then the editor (see [The editors](#the-editors)) |
| `COMMAND` | the `:` command line or a `/` search line is open | the `command_line` table, then [line editing](#line-editing) |
| `PICKER` | a picker is open | the `picker` table, then [line editing](#line-editing) of the query |
| `VIEW NORMAL`, `VIEW INSERT`, `VIEW VISUAL`, `VIEW SEARCH` | the message view, or `:help`, `:map` or `:messages` in it, is open; the mode of its editor | the `message_view` table in normal and visual mode, then the editor |
| `CONFIRM` | an action asks first, as `dd` does, or `:join` before it creates a direct message | `y` confirms, any other key cancels |
| `SECRET` | `:recover` asks for the recovery key; the status line shows `Recovery key:` and a `*` for each character, with the cursor among them | [line editing](#line-editing) and a paste's first line; `<CR>` submits, `<Esc>` cancels; no keymap |

In `NORMAL`, the room list and the timeline have no character cursor: the
cursor moves one room or one message at a time. That leaves `<Space>` free
for the leader.

Counts work in the room list and the timeline: `5j` moves five messages
down. A count applies to `j`, `k`, `<C-d>` and `<C-u>`; `0` only continues
a count that has started.

### Line editing

The command line, the `/` search lines, picker queries and the recovery key
prompt have a cursor and these keys, without history. In `COMMAND` and
`PICKER` mode the `command_line` and `picker` tables come first: a key bound
there (`"<C-e>" = "picker.cancel"`) never reaches the line.

| Key | Effect |
|---|---|
| a character | inserted at the cursor |
| `<Left>` / `<Right>` | one character left / right; an emoji with a skin tone is one character |
| `<Home>`, `<C-b>` / `<End>`, `<C-e>` | to the start / end |
| `<S-Left>` / `<S-Right>` | to the start of the word before the cursor / the end of the word after it, over the blanks in between |
| `<BS>` / `<Del>` | deletes the character before / after the cursor; `<BS>` on an empty command or search line closes it |
| `<C-w>` | deletes the word before the cursor, with any blanks between them |
| `<C-u>` | deletes everything before the cursor |

Other keys do nothing there, and a paste goes in at the cursor, up to its
first line break. Words are split as the editors' `w` and `b` split them: a
word is a run of letters, digits and `_`, of punctuation and symbols (emoji
included), of kanji, of hiragana, of katakana or of hangul, and blanks (the
ideographic space too) separate words. `<C-w>` at the end of
`open foo.bar` deletes `bar`, then `.`, then `foo`, then `open `, and at the
end of `日本語のテキスト` it deletes `テキスト`, then `の`, then `日本語`.

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
| `<C-w>h`, `<C-w><C-h>` | `focus.room_list` | focus the room list (and show it) |
| `<C-w>l`, `<C-w><C-l>` | `focus.timeline` | focus the timeline |
| `<C-w>j`, `<C-w><C-j>` | `composer.focus` with `mode = "keep"` | focus the composer below the timeline, in the mode it was left in |
| `<C-w>k`, `<C-w><C-k>` | `focus.timeline` | focus the timeline above the composer |
| `]u` / `[u` | `room.open` with `filter = "unread"` and `pick = "next"` / `"prev"` | open the next or previous unread room, wrapping around |
| `<C-o>` / `<Tab>` | `jump.back` / `jump.forward` | back to where the cursor was before a jump, and forward again (see [The jump list](#the-jump-list)); `<C-i>` is `<Tab>` |

As in nvim, ctrl can stay held for the key after `<C-w>`.

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
| `composer.normal` | `<Esc>` | `composer.leave` | back to the timeline; the text stays, and a reply or an edit without text ends |
| `composer.normal` | `<C-c>` | `composer.cancel` | end a reply (the text stays) or an edit (the text goes) |
| `composer.completion` | `<C-n>`, `<Down>` / `<C-p>`, `<Up>` | `completion.next` / `completion.prev` | select a mention |
| `composer.completion` | `<C-y>`, `<Tab>`, `<CR>` | `completion.accept` | insert it |
| `composer.completion` | `<C-e>` | `completion.cancel` | close the mention menu |

`<Esc>` in insert mode is not bound: the editor goes to normal mode, and a
second `<Esc>` returns to the timeline.

### `picker`, `command_line` and `message_view`

| Table | Key | Action |
|---|---|---|
| `picker` | `<C-n>`, `<Down>`, `<Tab>` / `<C-p>`, `<Up>`, `<S-Tab>` | `picker.next` / `picker.prev` |
| `picker` | `<CR>` / `<Esc>` | `picker.accept` / `picker.cancel` |
| `command_line` | `<CR>` / `<Esc>` / `<Tab>` | `command_line.execute` / `command_line.cancel` / `command_line.complete` |
| `message_view` | `q`, `<Esc>` | `view.close` |

### Actions without a default key

These actions have no default binding; bind them in the configuration, or
use the commands below:

| Action | Command |
|---|---|
| `room.open` with other arguments than `]u` and `[u`, for example a `query` | `:open <room>` |
| `room.mark_read` | |
| `room.mute`, `room.unmute` | `:mute`, `:unmute` |
| `room.join`, with a `target` | `:join <target>` |
| `message.react` with an `emoji`, for example `emoji = "thumbsup"` | `:react <emoji>` |
| `file.upload` | `:upload <path>` |
| `account.recover`, which asks for the recovery key | `:recover` |
| `help.commands`, the command list | `:help` |
| `help.keymap`, the bindings of each table | `:map` |
| `status.messages`, the status line's recent messages | `:messages` |
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

Show the bindings with `<leader>sk`, and react with 👍 on the message under
the cursor with `<leader>+`:

```toml
[account]
user_id = "@alice:example.org"

[keymap.normal]
"<leader>sk" = "help.keymap"

[keymap.timeline]
"<leader>+" = { action = "message.react", emoji = "thumbsup" }
```

## The jump list

As in nvim, `<C-o>` goes back to where the timeline cursor was before a
jump, also in another room, and `<Tab>` forward again. These are jumps:

- opening another room, whichever way: `<CR>` in the room list, the room
  picker, `]u` and `[u`, `:open` and `:join`;
- `gg` and `G` in the timeline;
- a search in the timeline with `/`, `n` or `N`;
- `gd`.

`j`, `k`, `<C-d>` and `<C-u>` are not jumps, nor are a search or `gg` in
the room list. Before each jump the list notes the room and the message
under the cursor (the room alone while its timeline is empty), keeps the
last 100 for as long as mxt runs, and does not note the same place twice
in a row. `<C-o>` first notes where you are, so `<Tab>` comes back there;
a jump after going back drops the places after the one you went back to.
Neither key takes a count.

Going back to a place opens its room, focuses the timeline and puts the
cursor on the message, after loading older messages when it is not
loaded; the status line says `Loading older messages…` meanwhile, and any
key stops the wait. A place in a room you left, or whose message is gone
(not there a second after the room's oldest message is loaded), is
skipped, and the status line says so, for example `Skipped a jump to a
room you left`. With nothing left in that direction nothing moves, and
the status line says `No older jump` or `No newer jump`.

Terminals send the same key for `<C-i>` and `<Tab>`, so in mxt, as in
nvim, `<C-i>` is another name for `<Tab>` (mxt does not turn on the kitty
keyboard protocol, which tells them apart). Both keys are in `normal`, so
they jump from the room list, the timeline and the composer's normal and
visual modes, which they leave for the timeline. Elsewhere they keep their
meaning: in the composer's insert mode both go to the editor, so `<Tab>`
inserts a tab; in the mention menu `<Tab>` inserts the mention and `<C-o>`
goes to the editor as in insert mode; in pickers `<Tab>` selects the next
item and on the command line it completes, while `<C-o>` does nothing
there; the message view passes both to its editor.

## Commands

`:` opens the command line. Everything after the command name is one
argument, so names and paths with spaces need no quotes. `<Tab>` completes
command names, room names after `:open`, the names of pending invites
after `:join`, and emoji shortcodes after `:react` (`:react thu` offers
`thumbsdown` and `thumbsup`).

| Command | What it does |
|---|---|
| `:open <room>` | Opens the room whose name matches `<room>` best (fuzzy). |
| `:join <target>` | Joins a room and opens it, or opens a direct message (see [Joining rooms and direct messages](#joining-rooms-and-direct-messages)). |
| `:mute` | Mutes the room: the room under the cursor in the room list, otherwise the open room. |
| `:unmute` | Unmutes it: its own notification setting is removed, so the account's default applies. A room that is not muted is left alone. |
| `:upload <path>` | Sends the file to the open room as an image, video, audio or file message, by the type its name suggests. `~/` is your home directory; a relative path is relative to where mxt was started. Anything that is not a regular file, and files larger than the homeserver's upload limit, are refused. |
| `:react [<emoji>]` | Toggles a reaction with the emoji on the message under the open room's timeline cursor: adds it, or removes yours. The emoji is an emoji (`:react 👍`) or a shortcode with colons on both sides or none (`:react thumbsup`, `:react :+1:`); an unknown one is an error in the status line, and nothing is sent. Without an emoji, it opens the emoji picker, as `+` does. With no open room, on an empty timeline or on a deleted message, both say so in the status line. |
| `:recover` | Asks for the recovery key in `SECRET` mode and, as on the first start, cross-signs this device with it and lets it fetch room keys from the key backup (see [First start](../README.md#first-start)). An empty key changes nothing. The status line then says `Recovery complete` or why it failed: a wrong key, or an account without secret storage, which needs recovery set up in another client first. |
| `:help` | Lists the commands, each with its usage and a one-line description, in the full-screen view. |
| `:map` | Lists the bindings of each keymap table after your configuration, as `keys -> action (arguments)`, in the full-screen view. A table shows its own bindings, not those of the table it falls back to, and a removed default shows as `nop`. |
| `:messages` | Lists the status line's messages of this run, oldest first, in the full-screen view: the last 200, errors and other messages alike, and at startup each warning about the configuration (see [Configuration](configuration.md#errors-and-warnings)) before the message that counts them. `G` goes to the newest. |
| `:q` | Quits. |

`:mute`, `:unmute`, `:recover`, `:help`, `:map`, `:messages` and `:q` take
no argument, `:open`, `:join` and `:upload` without one show their usage,
and `:react`'s argument is optional. `:react` acts on the open room's
timeline cursor also while the room list has focus, whereas `:mute` and
`:unmute` there act on the room under the room list's cursor.

### Joining rooms and direct messages

`:join` reads its argument as one of these:

| Argument | What happens |
|---|---|
| a room alias, `#name:server` | Joins the room and opens it. |
| a room ID, `!id:server` | Joins the room, which also accepts a pending invite to it, and opens it; a room in the room list opens at once. |
| a user ID, `@name:server` | Opens your direct message with that person, below. |
| a matrix.to link, `https://matrix.to/#/` and one of the above, with or without `?via=<server>` | As for what it links to; a link to a message opens its room. The `via` servers help your homeserver find a room it is not in. |
| anything else | The name of a pending invite, matched as `:open` matches room names: joins it, which accepts the invite. When two invites have that name, `<Tab>` completes their room IDs instead. |

The status line says `Joining…`, then `Joined <room>` when the room opens,
or why it failed. A room you are already in just opens. A key you press
before the answer arrives keeps the room from opening under you; it still
appears in the room list. mxt does not list spaces: a space you are in is
an error, and `:join` of another space joins it, then says that mxt does
not list it.

`:join @name:server` looks for a direct message with that person, in this
order, and opens the first room it finds (the most recently active one
when a step finds several):

1. a room in your direct messages (`m.direct`) that they have not left,
   or one this run of mxt created with them;
2. a room only you and they have joined; with bridge state only when they
   are the bridge's bot, the room has no name and the bridge declares it
   no room type or a `dm`, which finds a bridge bot's room;
3. a bridged direct message with them, as the room list names it;
4. their pending invite to a direct message, which it accepts.

Only when none is found does it ask `Create a direct message with
<user>? (y/n)`. `y` creates an encrypted direct message and invites them,
and the status line says `Created a direct message` when it opens; any
other key cancels, and nothing is created. A key you press before the
question would appear means no question: the status line says `No direct
message found; :join again to create one`. Opening an existing direct
message and joining a room by its alias, ID or link never ask.

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
  `<C-c>` in normal mode ends the reply or the edit. `<Esc>` in normal mode
  ends it too when the composer is empty (a space counts as text), so a
  reply started by mistake ends with `<Esc>` and nothing typed is lost.

The message view (`<CR>` on a message) holds the message's text in an
editor, so motions, `/` search, visual selection and yanks work on messages
taller than the screen. `i` and other edits work too, but are discarded when
the view closes. `q` and `<Esc>` close it in normal and visual mode. `:help`,
`:map` and `:messages` show their text in the same view.

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
  keymap: `f:` finds a colon, and `<Esc>` cancels. As in nvim's
  operator-pending mode, so does the key after an operator (`d`, `c`, `y`)
  and after a text object's `i` or `a` (`ci[`, `vi"`): `d<Esc>` cancels the
  `d`, and a binding in `normal` such as `]u` never takes the `]` of `ci]`.
  As in nvim, so does the key after `g`: the editors' only `g` command is
  `gg`, any other key after `g` is dropped with it (`g]`, `gx`), and
  `g<Esc>` cancels the `g`.
- `df`, `dt`, `cf` and `ct` work, and `;` and `,` repeat their find. Other
  operators with these motions (`dF`, `cT`, `yf`, `d;`) do nothing, and the
  character after `dF` or `yf` is dropped, as are the one after `r` in
  visual mode and the key after `gf`, `gF`, `gt` or `gT`.
- The text objects are `i` with `w`, `W`, a quote or a bracket, after `d`
  and `c` and in visual mode (`viW` excepted). There are no `a` objects and
  no `yi`: `da(`, `yi"` and visual `a(` do nothing, and the key after them
  is dropped.
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

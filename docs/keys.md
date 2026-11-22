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
  room's timeline is on the right, with the composer below it. `t` shows a
  message's thread in the timeline's place (see [Threads](#threads)).
- The room list is in the order of `ui.room_list.sort`, by latest activity
  unless it says otherwise (see [Sort keys](configuration.md#sort-keys)).
  Opening a room, with `<CR>` or any other way of opening it (`]u`, `[u`,
  the room picker, `:open`, `:join`, the jump list), puts the cursor on
  it; opening the room that is already open only moves the focus to its
  timeline, and the cursor stays where it is. The first time the opened
  room then moves in the list because of a change to it, while the cursor
  is still on it (it sorts as read as it opens, you read it, a message
  arrives in it, it is renamed), the cursor goes to the room that came
  right after it, or to the room before it if it was the last, as in a
  mail inbox: with
  `sort = "mentions, notify, unread, read"`, `<CR>` or `]u` opens the top
  room, it drops out of its group, and the cursor is on the next one; with
  `sort = "read"`, `<CR>` on an unread room leaves it in place, reading it
  lifts it to the top, and the cursor goes to the room that was right
  below it. Another room passing the open room does not count, nor does a
  room added or removed above it or the room you left going back above
  it: the cursor stays on the open room until the open room itself moves.
  After that the cursor stays on its room as rooms move, and so it does
  when you moved the cursor off the open room before the room moved, also
  if you moved it back. A room you left unread (the terminal had no focus)
  goes back to its place among the unread rooms once you open another.
  `]u`, `[u` and the room picker follow the list's order.
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
arrive) and `(unsupported: …)`. With `ui.timeline.images = "on"` (see
[Configuration](configuration.md#uitimeline)), an image's or a sticker's
preview follows its line, at most `ui.timeline.image_rows` rows tall.
A message taller than the pane is cut with `[+N lines]`; `<CR>` shows it in
full. Date dividers and a `new messages` line, where you stopped reading,
come between messages.
Replies in threads are not in the timeline. Their root ends with a dim row
`💬 3 · 14:32`: the number of replies and the time of the latest, with its
date when it is on another day than the root, or `💬 3` alone until mxt
has loaded the latest reply. The row ends with `· unread` while the room's
unread count includes replies in that thread: replies newer than what mxt
last marked read in the room since you opened it (the timeline's newest
message once the pane shows it, or `room.mark_read`), in a room that has
unread messages.
A thread shown in place of the timeline has a title row under the room's
name, `Thread · Alice: first line`: the root's sender and the first line of
its text, or `Thread` alone while mxt has not loaded the root. Below it
come the root, with its `💬` row but never `· unread`, and the replies.
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

Counts work in the room list and the timeline, a thread's too: `5j` moves
five messages down. A count applies to `j`, `k`, `<C-d>` and `<C-u>`; `0`
only continues a count that has started.

### Line editing

The command line, the `/` search lines, picker queries and the recovery key
prompt have a cursor and these keys. The command line and the `/` search
lines also have a [history](#history). In `COMMAND` and `PICKER` mode the
`command_line` and `picker` tables come first: a key bound there
(`"<C-e>" = "picker.cancel"`) never reaches the line.

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

#### History

`<Up>` and `<Down>` on the command line and the `/` search lines recall
the lines you ran there. The first `<Up>` takes the text before the cursor
and puts the newest line that starts with it in the line, the cursor at its
end; each further `<Up>` goes to the next older such line, and `<Down>`
back to the newer ones. `<Down>` past the newest brings back what you
typed, with the cursor where it was. The match is case-sensitive, and with
nothing before the cursor every line matches. Typing, deleting, moving the
cursor, `<Tab>` or a paste ends the browsing, so the next `<Up>` looks for
the text then.

Commands and searches have separate histories of 100 lines each, and the
room list's and the timeline's searches share one. A line goes in when you
run it with `<CR>`, also when the command fails; a line you cancel with
`<Esc>`, an empty line and a command line of blanks only do not (a search
of blanks is a pattern and goes in), and a line you ran before moves to the
newest place. mxt keeps both in `$XDG_STATE_HOME/mxt/history` (mode 0600)
across starts. Picker queries and the recovery key have no history: after
`:recover`, only `recover` goes in, and a `:recover` line with an argument,
which may be the key itself, does not go in at all.

## Default keymap

`normal` applies wherever the focused table does not bind the key, outside
text input: in the room list, the timeline, and the composer in normal and
visual mode. In a thread, `thread` comes first, then `timeline`, then
`normal`.

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
| `]u` / `[u` | `room.open` with `filter = "unread"` / the same with `pick = "prev"` | open the topmost unread room other than the open one / the previous unread room, wrapping around |
| `<C-o>` / `<Tab>` | `jump.back` / `jump.forward` | back to where the cursor was before a jump, and forward again (see [The jump list](#the-jump-list)); `<C-i>` is `<Tab>` |

As in nvim, ctrl can stay held for the key after `<C-w>`.

`]u` and `[u` count every badge, dim counts included. Bound with
`filter = "notify"`, they skip the rooms whose only news is a dim count
(see [Sort keys](configuration.md#sort-keys)). A room you open but have
not read yet (the terminal has no focus, or you press `]u` again before
its timeline is drawn), or whose only unread messages are replies in
threads you have not shown, is still unread: `]u` skips it while it is
open, and once `]u` has opened another room it is among the unread rooms
again, so `]u` pressed again goes back and forth between the two topmost
unread rooms.

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

After `/`, `n` or `N`, in either pane, every match of the pattern in the
messages on the timeline's pane is shown in reverse video, as nvim's
`hlsearch` does, and the matches in the message under the cursor are bold
too. In inline code, which is drawn reversed, a match is underlined instead,
and a spoiler that holds a match is drawn as `▒` instead of `█`, on all of
its lines. `:noh` (or `:nohlsearch`) hides the highlight until the next
search, and so does opening another room; opening or closing a thread keeps
it. The room list's names are not highlighted. Matches in the cut-off part
of a tall message (`[+N lines]`) are not shown, so open the message with
`<CR>`: its view underlines them in bold, and a visual selection there stays
reversed. The view keeps the marks it opened with: clearing the highlight
while it is open, or editing its text, leaves them where they were. The
editors' own `/` shows only the current match, while you type it.

### `room_list`

| Key | Action | |
|---|---|---|
| `<CR>` | `room.open_selected` | open the room under the cursor |

### `timeline`

| Key | Action | |
|---|---|---|
| `i`, `a` | `composer.focus` | write in insert mode |
| `<CR>` | `message.view` | show the message in full, or the image (of a sticker too) |
| `r` | `message.reply` | reply |
| `e` | `message.edit` | edit your own text message (the composer must be empty) |
| `dd` | `message.delete` | delete your own message, after `y` |
| `yy` | `message.yank` | copy the message's text |
| `+` | `message.react` | react with the emoji picker; an emoji you already chose removes your reaction |
| `gx` | `message.open` | open the link, or download and open the attachment |
| `gd` | `message.goto_reply` | go to the message this one replies to |
| `R` | `message.resend` | send a message marked `failed` again |
| `t` | `thread.open` | show the message's thread in place of the timeline (see [Threads](#threads)) |

A message that failed for a passing reason, such as a lost connection, also
goes out by itself once the sync is back; `R` is for the others.

`t` did nothing in the timeline before mxt had threads. It is in
`timeline`, which comes before `normal`, so if your configuration binds
`t` in `[keymap.normal]`, write that binding in `[keymap.timeline]` too to
keep it in the timeline.

### `thread`

These keys apply while a thread is shown in place of the timeline and the
timeline has focus; every other key is looked up in `timeline`, then in
`normal`.

| Key | Action | |
|---|---|---|
| `q`, `<Esc>` | `thread.close` | back to the timeline, with the cursor on the thread's root |

The timeline binds neither key, so a binding of `q` or `<Esc>` in
`[keymap.normal]` still applies there.

### `composer.insert`, `composer.normal` and `composer.completion`

| Table | Key | Action | |
|---|---|---|---|
| `composer.insert` | `<CR>` | `composer.send` | send |
| `composer.insert` | `<C-j>` | `composer.newline` | a line break (many terminals send Shift+Enter as `<CR>`) |
| `composer.insert` | `<C-c>` | `composer.normal` | normal mode, as vim's `<C-c>` |
| `composer.insert` | `<C-w>`, `<A-BS>` | `composer.delete_word_before` | delete the word before the cursor, as vim's `<C-w>` |
| `composer.insert`, `composer.normal` | `<C-x><C-e>` | `composer.external_editor` | edit in `$VISUAL` or `$EDITOR` |
| `composer.normal` | `<Esc>` | `composer.leave` | back to the timeline; the text stays, and a reply or an edit without text ends |
| `composer.normal` | `<C-c>` | `composer.cancel` | end a reply (the text stays) or an edit (the text goes) |
| `composer.completion` | `<C-n>`, `<Down>` / `<C-p>`, `<Up>` | `completion.next` / `completion.prev` | select a mention |
| `composer.completion` | `<C-y>`, `<Tab>`, `<CR>` | `completion.accept` | insert it |
| `composer.completion` | `<C-e>` | `completion.cancel` | close the mention menu |

`<Esc>` in insert mode is not bound: the editor goes to normal mode, and a
second `<Esc>` returns to the timeline. `<C-c>` in insert mode goes to
normal mode too, and a second `<C-c>` ends a reply or an edit. Up to mxt
0.1.4, `<C-c>` did nothing in insert mode; `"<C-c>" = "nop"` in
`[keymap.composer.insert]` keeps it that way.

`<C-w>` and `<A-BS>` in insert mode delete the word before the cursor and
the blanks after it, split into words as the command line's `<C-w>` splits
them (see [Line editing](#line-editing)); at the start of a line they delete
the line break, joining the line to the one above. `<C-w>` in normal mode is
still the window prefix (`<C-w>h`, `<C-w>j`, …). `<C-BS>` is not a default,
because most terminals send it as `<C-h>` or `<BS>`; if yours tells it
apart, bind it with `"<C-BS>" = "composer.delete_word_before"` in
`[keymap.composer.insert]`. Up to mxt 0.1.4, `<C-w>` and `<A-BS>` did
nothing in insert mode; `"<C-w>" = "nop"` and `"<A-BS>" = "nop"` in
`[keymap.composer.insert]` keep it that way.

### `picker`, `command_line` and `message_view`

| Table | Key | Action |
|---|---|---|
| `picker` | `<C-n>`, `<Down>`, `<Tab>` / `<C-p>`, `<Up>`, `<S-Tab>` | `picker.next` / `picker.prev` |
| `picker` | `<CR>` / `<Esc>` | `picker.accept` / `picker.cancel` |
| `command_line` | `<CR>` / `<Esc>` / `<Tab>` | `command_line.execute` / `command_line.cancel` / `command_line.complete` |
| `command_line` | `<Up>` / `<Down>` | `command_line.history_prev` / `command_line.history_next` (see [History](#history)) |
| `message_view` | `q` | `view.close` |
| `message_view` | `<Esc>` | `view.leave`: in visual mode back to normal mode, otherwise `view.close` |

Up to mxt 0.1.4, `<Esc>` closed the message view in visual mode too;
`"<Esc>" = "view.close"` in `[keymap.message_view]` keeps it that way.

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
| `composer.clear`, which empties the composer as one change that `u` undoes and keeps a reply or an edit | |
| `file.upload` | `:upload <path>` |
| `account.recover`, which asks for the recovery key | `:recover` |
| `help.commands`, the command list | `:help` |
| `help.keymap`, the bindings of each table | `:map` |
| `status.messages`, the status line's recent messages | `:messages` |
| `search.clear_highlight`, which hides the search highlight until the next search | `:noh`, `:nohlsearch` |
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

## Threads

`t` (`thread.open`) shows the thread of the message under the cursor in
place of the timeline: the root, then its replies, oldest first, under the
title row `Thread · <sender>: <first line>`. On a message without replies
the thread is new and empty, and the first message you send there starts
it. `t` refuses a message that is still being sent (`Wait until the
message is sent`) and a deleted message without replies (`The message is
deleted`); in a thread it does nothing more.

Reading a thread sends a read receipt for its newest reply once the pane
shows it, as the timeline does for its newest message, and other clients
show the thread as read. When you have read the timeline (to its newest
message, which is older than the reply), that receipt also marks the room
read up to the reply: the room's badge goes, and the older replies of other
threads count as read too, in mxt and in other clients, as reading the
timeline does. Otherwise mxt keeps counting the thread's replies in the
room's badge, and keeps its root's `· unread`, until you read the timeline
past them or use `room.mark_read`.

In a thread:

- The timeline's keys work on the thread's messages: moving and counts,
  `/`, `n`, `N`, `<CR>`, `e`, `dd`, `yy`, `+`, `gx`, `R`, `i` and `a`, and
  older replies load as the cursor nears the top. `room.mark_read`,
  `:mute` and `:unmute` act on the room.
- The composer sends into the thread. Each thread has its own draft, as
  each room has: what you leave in a thread's composer waits there for the
  next time you open the thread, and the room's draft, reply or edit is
  back when you leave the thread. Clients without threads show a message
  sent into a thread as a reply to the thread's previous message.
- `r` replies within the thread to the message under the cursor.
- `:upload` sends the file into the thread.
- `gd` goes to the replied-to message in the thread, or in the timeline,
  which closes the thread.
- `q` or `<Esc>` (`thread.close`) shows the timeline again with the cursor
  on the root. When the timeline no longer holds the root, older messages
  load until it does; the status line says `Loading older messages…`, and
  any key stops the wait. When the root is gone (not there a second after
  the timeline's oldest message is loaded), the cursor follows the newest
  message.
- Opening another room leaves the thread.

In the timeline, `gd` on a reply to a message in a thread (sent from a
client without threads) opens that thread with the cursor on the message,
loading older replies until it arrives. A message that neither the
timeline nor a thread that mxt knows holds is not loaded, as before.

Opening and closing a thread are jumps (see [The jump list](#the-jump-list)):
`<C-o>` after `t` goes back to the root in the timeline, and `<Tab>` into
the thread again.

## The jump list

As in nvim, `<C-o>` goes back to where the timeline cursor was before a
jump, also in another room or thread, and `<Tab>` forward again. These are
jumps:

- opening another room, whichever way: `<CR>` in the room list, the room
  picker, `]u` and `[u`, `:open` and `:join`;
- opening a thread with `t`, and closing it with `q` or `<Esc>`;
- `gg` and `G` in the timeline;
- a search in the timeline with `/`, `n` or `N`;
- `gd`.

`j`, `k`, `<C-d>` and `<C-u>` are not jumps, nor are a search or `gg` in
the room list. Before each jump the list notes the room, the thread while
one is shown, and the message under the cursor (the room or thread alone
while its timeline is empty), keeps the last 100 for as long as mxt runs,
and does not note the same place twice in a row. `<C-o>` first notes where
you are, so `<Tab>` comes back there; a jump after going back drops the
places after the one you went back to. Neither key takes a count.

Going back to a place opens its room, and its thread or the timeline,
focuses the timeline and puts the cursor on the message, after loading
older messages when it is not loaded; the status line says `Loading older
messages…` meanwhile, and any key stops the wait. A place in a room you
left, or whose message is gone (not there a second after the oldest
message of its timeline or thread is loaded), is skipped, and the status
line says so, for example `Skipped a jump to a room you left`. With nothing
left in that direction nothing moves, and the status line says `No older
jump` or `No newer jump`.

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
| `:upload <path>` | Sends the file to the open room, or into the thread shown in place of its timeline, as an image, video, audio or file message, by the type its name suggests. `~/` is your home directory; a relative path is relative to where mxt was started. Anything that is not a regular file, and files larger than the homeserver's upload limit, are refused. |
| `:react [<emoji>]` | Toggles a reaction with the emoji on the message under the open room's timeline cursor: adds it, or removes yours. The emoji is an emoji (`:react 👍`) or a shortcode with colons on both sides or none (`:react thumbsup`, `:react :+1:`); an unknown one is an error in the status line, and nothing is sent. Without an emoji, it opens the emoji picker, as `+` does. With no open room, on an empty timeline or on a deleted message, both say so in the status line. |
| `:recover` | Asks for the recovery key in `SECRET` mode and, as on the first start, cross-signs this device with it and lets it fetch room keys from the key backup (see [First start](../README.md#first-start)). An empty key changes nothing. The status line then says `Recovery complete` or why it failed: a wrong key, or an account without secret storage, which needs recovery set up in another client first. |
| `:help` | Lists the commands, each with its usage and a one-line description, in the full-screen view. |
| `:map` | Lists the bindings of each keymap table after your configuration, as `keys -> action (arguments)`, in the full-screen view. A table shows its own bindings, not those of the table it falls back to, and a removed default shows as `nop`. |
| `:messages` | Lists the status line's messages of this run, oldest first, in the full-screen view: the last 200, errors and other messages alike, and at startup each warning about the configuration (see [Configuration](configuration.md#errors-and-warnings)) before the message that counts them. `G` goes to the newest. |
| `:noh`, `:nohlsearch` | Hides the search highlight until the next search (see [`room_list` and `timeline`](#room_list-and-timeline)). |
| `:q` | Quits. |

`:mute`, `:unmute`, `:recover`, `:help`, `:map`, `:messages`, `:noh`,
`:nohlsearch` and `:q` take no argument, `:open`, `:join` and `:upload`
without one show their usage, and `:react`'s argument is optional.
`:react` acts on the open room's timeline cursor also while the room list
has focus, whereas `:mute` and `:unmute` there act on the room under the
room list's cursor.

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
  line break, and `<C-x><C-e>` edits it in `$VISUAL` or `$EDITOR`. After
  sending, the composer stays in insert mode; to be in normal mode after
  sending, also when a blank text sends nothing, bind `<CR>` to both:

  ```toml
  [account]
  user_id = "@alice:example.org"

  [keymap.composer.insert]
  "<CR>" = ["composer.send", "composer.normal"]
  ```
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
the view closes. `q` closes it in normal and visual mode, and `<Esc>` in
normal mode; in visual mode `<Esc>` goes back to normal mode with the cursor
where it is, as in vim, so after `V`, `<Esc>v` starts a selection of
characters. `:help`, `:map` and `:messages` show their text in the same
view.

In both editors, in insert and normal mode, `<Up>` on the first line goes to
the start of the text and `<Down>` on the last line to its end, as in a text
field (in normal mode onto the last character, an emoji with a skin tone
whole); on the other lines they go a line up or down. A line is a line of
the text, wrapped on the screen or not, so `<Up>` anywhere on a wrapped
first line goes to the start. `k` and `j`, and `<Up>` and `<Down>` in visual
mode, stop at the first and the last line, as in vim. While the composer's
mention menu is open, `<Up>` and `<Down>` select a mention instead.

Yanks (`yy` in the timeline, `y` in the editors) go to one register that
every editor shares, and to the terminal's clipboard
([Terminal setup](terminal.md#clipboard)). `p` pastes from that register, not
from the system clipboard; the terminal's own paste goes in as text, line
breaks included, and runs no keys.

`"_` before a command in normal and visual mode deletes or yanks without
writing that register or the terminal's clipboard, as vim's black hole
register does: `"_dd`, `"_diw`, `"_cw`, `"_D`, `"_yy`, and in visual mode
`"_d` and `"_c`. It counts for that one command. Other registers are not
supported: `"a`, `"+` and the like do nothing, and the status line says so.
`"_p` pastes the register as `p` does.

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
- In insert and normal mode, `<Up>` on the first line and `<Down>` on the
  last go to the start and the end of the text, as in a text field, where
  vim stays on the line (see [The editors](#the-editors)).
- Registers: the editors share one register (see above), and `"_` is the
  only register name. `x` writes no register, so `x` and `"_x` are the
  same, and `"_p` pastes the register, where vim's black hole register is
  empty. `.` after `"_dd` deletes into the register, where vim repeats
  with `"_`; type `"_.` to repeat without writing. The key after `"`, and
  the keys of the command after `"_`, go to the editor without the keymap.
- Not handled yet: `x`, `r`, `a`, the arrow keys in insert mode and visual
  selections work on single characters inside a grapheme cluster, so they
  can split an emoji; `e` stops on the last character of a cluster (the
  variation selector of `❤️`); `df` and `dt` can split a cluster (`df👍`
  leaves the skin tone); `df<Tab>` cancels, where `f<Tab>` finds a tab;
  operators with `F`, `T`, `;` and `,`; a linewise `p` on the last line
  pastes above it; `.` does not repeat `r`, and after an insert it repeats
  the typed characters but not what `<BS>`, `<C-w>`, `<A-BS>` or
  `composer.clear` deleted; `u` after `composer.clear`, or after `<BS>`,
  `<C-w>` or `<A-BS>` at the end of a line, puts the normal-mode cursor one
  column past the line's last character, so `h` then goes to the
  second-to-last; `r` in visual mode.

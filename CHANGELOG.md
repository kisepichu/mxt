# Changelog

All notable changes to mxt are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
the rules in [Releasing](docs/releasing.md): Semantic Versioning 0.x, where a
change that breaks existing setups bumps the minor version. Such changes are
marked **Breaking:**.

## [Unreleased]

### Added

- The `notify` sort key for `ui.room_list.sort` puts the rooms that notify
  you first: unread mentions, bold counts and rooms marked unread, the
  rooms that `filter = "notify"` takes. With
  `sort = "mentions, notify, unread, read"`, rooms whose only news is a dim
  count (muted rooms, rooms set to mentions and keywords only) come after
  them, and `"]u" = { action = "room.open", filter = "notify" }` in
  `[keymap.normal]` jumps between the rooms that notify only. As with
  `mentions` and `unread`, the room you have open sorts as read. `unread`
  and `filter = "unread"` are unchanged. mxt 0.1.4 skips `notify` with a
  warning and sorts by the other keys.
- `composer.normal` puts the composer's editor in normal mode, and `<C-c>`
  in insert mode runs it, as vim's insert-mode `<C-c>` does; `<C-c>` in
  normal mode still ends a reply or an edit. With
  `"<CR>" = ["composer.send", "composer.normal"]` in
  `[keymap.composer.insert]`, the editor is in normal mode after sending,
  also when nothing was sent. `<C-c>` did nothing in insert mode before;
  `"<C-c>" = "nop"` in `[keymap.composer.insert]` keeps that.
- `composer.delete_word_before` deletes the word before the cursor, with
  the blanks after it, in the composer's insert mode, split into words as
  the command line's `<C-w>` splits them (kanji, hiragana, katakana, and
  punctuation with emoji are words of their own); `<C-w>` and `<A-BS>` in
  insert mode run it, and `<C-w>` in normal mode is still the window
  prefix. Many terminals send `<C-BS>` as `<C-h>` or `<BS>`, so it is not
  a default; bind it if yours tells it apart. These keys did nothing in
  insert mode before; `"<C-w>" = "nop"` and `"<A-BS>" = "nop"` in
  `[keymap.composer.insert]` keep that.
- `"_` before a command in the editors' normal and visual mode (`"_dd`,
  `"_diw`, `"_x`, `"_cw`, `"_D`, visual `"_d`) deletes or yanks without
  writing mxt's register or the terminal's clipboard, as vim's black
  hole register; `.` repeats such a change with a write, so use `"_.`
  to repeat it without one. Other registers are not supported, and the
  status line says so. `composer.clear` empties the composer as one
  change that `u` undoes and keeps a reply or an edit; it has no
  default key.

### Changed

- The `read` sort key orders unread rooms by the time of their newest
  message and the other rooms by your last read, newest first, so with
  `sort = "mentions, unread, read"` (or `"mentions, notify, unread, read"`)
  a room that gets a new message goes to the top of its group; before,
  unread rooms went by your last read too, and a room unread after days of
  silence sat low in its group. The room you have open counts as read up
  to its newest message, and a room you read with something new goes to
  the top of the read rooms at once instead of waiting for the homeserver
  to confirm the receipt.
- `]u` opens the topmost unread room in the room list's order other than
  the one you have open, so a room with a new mention at the top comes
  first; it used to open the next unread room after the open one, which
  `"]u" = { action = "room.open", filter = "unread", pick = "next" }` in
  `[keymap.normal]` brings back. A room you opened but have not read yet
  (the terminal had no focus, or you pressed `]u` again before its
  timeline was drawn) stays unread, so `]u` pressed again goes back and
  forth between the two topmost unread rooms. `pick = "first"` of
  `room.open` now skips the open room (before, it could pick the open
  room, which only moved the focus to its timeline), and `]u` is
  `{ action = "room.open", filter = "unread" }`. `[u` is unchanged.
- In the composer's insert and normal mode, `<Up>` on the first line moves
  to the start of the text and `<Down>` on the last line to its end, as in
  a text field; `k` and `j` are unchanged, and the mention menu keeps its
  `<Up>` and `<Down>`. The message view's editor does the same.

### Security

- The `url` crate is at 2.5.9, which fixes
  [RUSTSEC-2026-0592](https://rustsec.org/advisories/RUSTSEC-2026-0592)
  (a stack overflow on nested `blob:` URLs).

## [0.1.4] - 2026-11-22

### Added

- `ui.room_list.sort` takes a comma-separated list of keys, applied in
  order, each deciding between the rooms the keys before it leave tied:
  `mentions` (rooms with unread mentions first), `unread` (rooms with
  unread messages first), `recent` (latest activity first), `read` (the
  rooms you read most recently first, by your own latest read receipt from
  any device; rooms you never read last) and `name`, for example
  `sort = "mentions, unread, read"`. Remaining ties go by name; `"recent"`
  and `"name"` mean what they did. With `read`, a room you open moves up
  when it had something new for you; with `mentions` or `unread`, the room
  you have open sorts as read until you open another. The cursor stays on
  its room when rooms move. mxt 0.1.3 ignores such a value with a warning
  and sorts by `recent`, so a configuration shared with it can carry it.
- Threads. `t` shows the thread of the message under the cursor in place of
  the timeline, or starts a thread on a message without replies; the
  timeline's keys work in it, the composer sends into it (`r` replies within
  the thread, `:upload` uploads into it), each thread keeps its own draft,
  and `q` or `<Esc>` goes back to the timeline with the cursor on the root.
  Opening and closing a thread are jumps for `<C-o>` and `<Tab>`, and `gd`
  goes to a replied-to message in a thread. Reading a thread sends a
  threaded read receipt, or, when you have read the timeline, an unthreaded
  one, so the room counts as read. The actions are `thread.open` and
  `thread.close`; the keymap table `thread` holds keys that apply only in a
  thread.

### Changed

- Replies in threads are no longer shown in the timeline. Their root shows
  `💬 N` with the time of the latest reply, and `· unread` while the room's
  unread count includes replies in that thread.
- `t` in the timeline opens a thread, so a binding of `t` in
  `[keymap.normal]` no longer applies while the timeline has focus; bind it
  in `[keymap.timeline]` too to keep it there.

### Fixed

- Moving the timeline cursor down with `j` or `<C-d>` near the newest message
  no longer jumps the view to the bottom with the cursor on the top row: the
  view scrolls just far enough to show the cursor's message, all the way to
  the newest. Messages that arrived below the pane meanwhile stay there,
  counted by `↓ N newer`, until the cursor reaches them or the pane grows
  (when a draft of several rows is sent, say).

## [0.1.3] - 2026-11-21

### Added

- Default keys: `]u` and `[u` open the next and previous unread room;
  `<C-w>j` focuses the composer in the mode it was left in and `<C-w>k` the
  timeline, and `<C-w>` works with ctrl held for `h`, `j`, `k` and `l`, as
  in nvim; `<Down>`, `<Tab>`, `<Up>` and `<S-Tab>` move through pickers, and
  `<Down>` and `<Up>` through the mention menu. Bindings of the same keys to
  the same actions in a configuration stay valid.
- The action `composer.leave`, which returns to the timeline and ends a
  reply or an edit whose text is empty.
- `:help` lists every command with a one-line description, and `:map`
  lists each keymap table's bindings after your configuration, as
  `keys -> action (arguments)`. Both open in the full-screen view, like a
  message, and `q` or `<Esc>` closes them. The actions `help.commands` and
  `help.keymap` open them from a key; they have no default key.
- `:react <emoji>` toggles a reaction on the message under the timeline's
  cursor, also from the room list. The argument is an emoji (`:react 👍`)
  or a shortcode (`:react thumbsup`, `:react :+1:`), and `<Tab>` completes
  shortcodes; `:react` without one opens the emoji picker, as `+` does.
- `message.react` takes an optional `emoji`, so a key can react with one
  emoji: `{ action = "message.react", emoji = "thumbsup" }`. An empty emoji
  is an error when the configuration is loaded; an unknown one ignores the
  binding with a warning.
- `:messages` lists the status line's recent messages (the last 200), the
  configuration's warnings included, in the full-screen view; the action
  `status.messages` opens it from a key.
- The command line, the `/` search line, picker queries and the recovery
  key prompt have a cursor: `<Left>`, `<Right>`, `<Home>` or `<C-b>`,
  `<End>` or `<C-e>`, `<S-Left>` and `<S-Right>` by word, and typing,
  `<BS>` and `<Del>` at the cursor. They work by grapheme cluster, so an
  emoji with a skin tone is one character, also for the recovery key's `*`.
- When newer messages are below the timeline pane, for example after a
  burst of messages arrived while the cursor was on an older one, the
  composer's top rule ends with `↓ N newer`; it goes once the pane shows
  the newest message.
- `:join <target>` joins a room by its alias (`#name:server`), its ID or a
  matrix.to link, which also accepts a pending invite, and opens it;
  `:join @name:server` opens the direct message with that person, bridged
  ones and a bridge bot's room included. Only when there is none does it
  ask whether to create one, and it creates an encrypted one after `y`.
  `<Tab>` after `:join ` completes pending invites by name. The action
  `room.join` takes `target`.
- A jump list, as in nvim: `<C-o>` goes back to where the timeline cursor
  was before a jump (opening another room, `gg`, `G`, `/`, `n`, `N`, `gd`),
  opening its room and loading older messages when needed, and `<Tab>`
  (`<C-i>`) goes forward again. The actions are `jump.back` and
  `jump.forward`.

### Changed

- The room list no longer labels rooms set to notify only for mentions and
  keywords `Mentions only`; muted rooms keep `Muted`.
- With the mention menu open, `<Down>` and `<Up>` select the next and
  previous candidate instead of moving the editor's cursor.
- `<Esc>` in the composer's normal mode runs `composer.leave`: it also ends
  a reply or an edit without text, so a reply started by mistake ends with
  `<Esc>`. Any text stays with its reply or edit. Binding `<Esc>` in
  `composer.normal` to `focus.timeline` keeps the old behavior.
- `+` with no open room or on an empty timeline now says why it does
  nothing: `No room is open` or `No message to react to`.
- Settings, tables, actions, action arguments, values and key names that
  this version does not know no longer stop mxt, nor does a
  `ui.time_format` it cannot read: each is ignored with a warning and its
  default applies, so one configuration works with older and newer
  versions. An unknown `ui.read_receipts` value sends private receipts.
  Syntax errors, wrong types, values out of range, conflicting or missing
  arguments and malformed keys still stop it. At startup the status line
  says how many settings were ignored, and the log names each with its
  line. A misspelled required key, such as `[acount]`, stops the start as
  a missing key, printed with the warning that names the misspelling.
- On the command line, the `/` search line, picker queries and the recovery
  key prompt, `<C-u>` deletes the text before the cursor and `<C-w>` the
  word before it, with words split as in the editors: `foo.bar` loses `bar`
  first, and kanji, hiragana and katakana are separate words.
- `<C-i>` in a binding is another name for `<Tab>`, as in nvim, since
  terminals send the same key for both: a `<C-i>` binding, which never ran,
  now runs on `<Tab>`. A table that binds both still loads; its `<C-i>`
  binding is ignored with a warning.

### Fixed

- After the timeline cursor moved up from the newest message (to reply to
  a message, for example), new messages, your own included, stayed below
  the pane until the cursor reached the newest message or the room was
  opened again. Now they appear at the bottom while the pane shows the
  newest message's last row, and whenever the cursor's message and every
  message after it fit on the pane.
- As in nvim's operator-pending mode, the key after `ci`, `di` and visual
  `i`, after `ca`, `da`, `yi`, `ya` and visual `a`, and after a bare `d`,
  `c` or `y` goes to the editor without the keymap. Bindings in `normal`
  that start with `[`, `]`, `(`, `"` and similar keys, the new `]u` and `[u`
  among them, no longer delay or take over text objects in the composer:
  `ci[` no longer waits for the next key, and a `u` typed after it no
  longer opens another room. `d<Esc>` cancels the `d` instead of leaving
  the composer.
- In the composer and the message view, the key after `g` goes to the
  editor without the keymap, as in nvim: `g]u` no longer opens another
  room, `g<Esc>` cancels the `g` instead of leaving the composer, and `gq`
  no longer closes the message view. Bindings in `normal` that start with
  `g`, `d`, `c` or `y` work as before, also when `gg` or `dd` is typed
  quickly before the next key: with `gt` bound, `ggx` runs `gg` and then
  `x`, and with `dq` bound, `ddj` deletes a line and moves down.

## [0.1.2] - 2026-11-20

### Changed

- Inside tmux, with `ui.image_preview = "auto"`, images are sixel that tmux
  itself places in the pane when tmux (3.6 or later, built with sixel) and
  the terminal support it. Otherwise kitty and Ghostty keep kitty graphics,
  which reach the terminal through tmux's `allow-passthrough`, and other
  terminals, WezTerm and iTerm2 too, get half blocks instead of iTerm2
  images, which tmux could not place. `"sixel"` inside tmux goes to tmux
  the same way wherever tmux supports it.

### Fixed

- Images inside tmux were drawn in the wrong place, over the file name, and
  left pieces behind after the message view closed.

## [0.1.1] - 2026-11-20

### Added

- `ui.image_preview` takes `"iterm2"`, `"sixel"`, `"kitty"` and
  `"halfblocks"`, which draw images in that protocol for a terminal where
  mxt's choice is wrong.

### Changed

- With `ui.image_preview = "auto"`, mxt asks the terminal for its name, or
  inside tmux asks tmux for the outer terminal's name, and draws images in
  iTerm2 graphics in WezTerm and iTerm2, and in kitty graphics in kitty and
  Ghostty; other terminals keep ratatui-image's detection. Inside WSL
  without SSH, images are half blocks.

### Fixed

- Images in WezTerm over SSH appear instead of boxes.

## [0.1.0] - 2026-11-19

The first release.

### Added

- Password login, session restore, and the recovery key on the first start or
  later with `:recover`, which cross-signs the device and fetches room keys
  from the key backup.
- A room list that names bridged rooms `<network> / <room>` and shows one
  unread badge per room (also for a room marked unread in another client),
  with the settings `ui.room_list.sort`, `ui.room_list.width` and
  `ui.room_list.network` (the network on the room's row or on a second row).
- A fuzzy room picker, the same picker for rooms with unread mentions, and an
  action that opens the next unread room.
- A timeline with HTML message bodies (spoilers hidden, in reply previews and
  code blocks too), replies, edits, reactions, read receipts and back
  pagination, and a full-screen message view for long messages and images.
- `/` searches the room list or the loaded messages, `n`/`N` repeat the
  search, and `yy` copies a message's text to the clipboard over OSC 52.
- The settings `ui.read_receipts` (`"private"` sends private read receipts),
  `ui.time_format` for message times, and `ui.image_preview` (`"off"` turns
  image previews off).
- Composers with vim modes, Markdown, `@` mention completion, replies, edits
  and an external editor. The editors use mxt's fork of edtui, which moves
  the cursor and wraps lines by grapheme cluster and uses vim's word classes
  (so word motions work on CJK text), and mxt adds `f`/`F`/`t`/`T` with `;`
  and `,`, and `r{char}`.
- Bracketed paste: a paste goes in as text and never sends a message or
  answers a question.
- Reactions from an emoji picker, editing and deleting your own messages, and
  resending a failed message with `R`; a send or edit that fails gives its
  draft back.
- Muting rooms, downloading and opening attachments, uploading files (images
  with their dimensions), and image previews in sixel, kitty or iTerm2
  graphics or half blocks, turned by their Exif orientation.
- A keymap in the configuration file: every key can be rebound, and a key can
  run any action or a sequence of actions.
- A Nix flake for `x86_64-linux`, `aarch64-linux` and `aarch64-darwin`, and
  prebuilt binaries for x86_64 Linux and Apple Silicon macOS on GitHub
  Releases, which `mise use -g github:kisepichu/mxt` installs.

[Unreleased]: https://github.com/kisepichu/mxt/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/kisepichu/mxt/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/kisepichu/mxt/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/kisepichu/mxt/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/kisepichu/mxt/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/kisepichu/mxt/releases/tag/v0.1.0

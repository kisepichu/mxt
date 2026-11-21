# Changelog

All notable changes to mxt are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
the rules in [Releasing](docs/releasing.md): Semantic Versioning 0.x, where a
change that breaks existing setups bumps the minor version. Such changes are
marked **Breaking:**.

## [Unreleased]

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

[Unreleased]: https://github.com/kisepichu/mxt/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/kisepichu/mxt/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/kisepichu/mxt/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/kisepichu/mxt/releases/tag/v0.1.0

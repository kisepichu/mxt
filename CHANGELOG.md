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

### Changed

- The room list no longer labels rooms set to notify only for mentions and
  keywords `Mentions only`; muted rooms keep `Muted`.
- With the mention menu open, `<Down>` and `<Up>` select the next and
  previous candidate instead of moving the editor's cursor.
- `<Esc>` in the composer's normal mode runs `composer.leave`: it also ends
  a reply or an edit without text, so a reply started by mistake ends with
  `<Esc>`. Any text stays with its reply or edit. Binding `<Esc>` in
  `composer.normal` to `focus.timeline` keeps the old behavior.

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

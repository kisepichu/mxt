# Changelog

All notable changes to mxt are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
the rules in [Releasing](docs/releasing.md): Semantic Versioning 0.x, where a
change that breaks existing setups bumps the minor version. Such changes are
marked **Breaking:**.

## [Unreleased]

## [0.1.0] - 2026-11-19

The first release.

### Added

- Password login, session restore, and the recovery key on the first start or
  later with `:recover`, which cross-signs the device and fetches room keys
  from the key backup.
- A room list that names bridged rooms `<network> / <room>` and shows one
  unread badge per room, with the settings `ui.room_list.sort`,
  `ui.room_list.width` and `ui.room_list.network` (the network on the room's
  row or on a second row).
- A fuzzy room picker, the same picker for rooms with unread mentions, and an
  action that opens the next unread room.
- A timeline with HTML message bodies (spoilers hidden, in reply previews and
  code blocks too), replies, edits, reactions, read receipts and back
  pagination, and a full-screen message view for long messages and images.
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

[Unreleased]: https://github.com/kisepichu/mxt/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/kisepichu/mxt/releases/tag/v0.1.0

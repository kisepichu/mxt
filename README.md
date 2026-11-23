# mxt

A terminal [Matrix](https://matrix.org) client with nvim-like keybindings,
made for accounts where most rooms are bridged from Discord, Slack and other
networks. Encryption, sync and unread counts come from
[matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk). Linux and
macOS.

## Features

- End-to-end encryption; the recovery key cross-signs the device and reads
  older messages from the key backup.
- Bridged rooms named `<network> / <room>` (`Discord / Alice`), with one
  unread badge per room.
- `]u` and `[u` jump between unread rooms; a fuzzy room picker, also for
  unread mentions.
- `:join` by alias, room ID, matrix.to link or invite, and direct messages by
  user ID, bridged ones included.
- A timeline with formatting, replies, edits, reactions, threads (`t`) and
  read receipts, and a full-screen view for long messages and images.
- A composer with vim modes, Markdown, `@` mention completion and an
  external editor.
- Reactions; editing, deleting and resending your messages; muting rooms;
  attachments both ways; images in sixel, kitty, iTerm2 graphics or half
  blocks.

Not included: desktop and typing notifications, SSO login, logout, several
accounts in one process, and setting up cross-signing or recovery for an
account that has none.

## Before you start

mxt is only the client. You also need:

- A Matrix account with a password: on matrix.org, a server from
  [servers.joinmatrix.org](https://servers.joinmatrix.org), or your own
  ([Synapse](https://github.com/element-hq/synapse),
  [Continuwuity](https://continuwuity.org)).
- Recommended: recovery set up in another client such as
  [Element](https://app.element.io). With its recovery key mxt reads older
  encrypted messages, and bridges that accept only cross-signed devices
  accept it; mxt cannot set up recovery itself.
- Optional: [bridges](https://docs.mau.fi/bridges/) for Discord, Slack and
  other networks. Most need a homeserver you administer: self-host one (for
  example with
  [matrix-docker-ansible-deploy](https://github.com/spantaleev/matrix-docker-ansible-deploy))
  or use a host that offers them, such as [etke.cc](https://etke.cc).

## Installing

- Prebuilt, for x86_64 Linux and Apple Silicon macOS:
  `mise use -g github:kisepichu/mxt`
- Nix: `nix profile install github:kisepichu/mxt`
- From source, with SQLite 3.54.0 or newer:
  `cargo install --locked --git https://github.com/kisepichu/mxt mxt`

Release archives, NixOS and home-manager, and build requirements:
[Installing](docs/installing.md).

## First start

1. Create `~/.config/mxt/config.toml` (`$XDG_CONFIG_HOME/mxt/config.toml`
   when set, on macOS too) with your user ID. `mxt` without it prints the
   path and this example:

   ```toml
   [account]
   user_id = "@alice:example.org"
   ```

   mxt finds the homeserver from the user ID (`.well-known`); if that fails,
   set `homeserver` in `[account]`. Everything else has a default; see
   [Configuration](docs/configuration.md).

2. Run `mxt`. It asks for your password once and saves the session.

3. Once the new device's keys reach the server, it asks for the recovery key
   (some clients call it the security key). An empty input skips it;
   `:recover` in the terminal UI asks again later.

4. `:q` quits. The keys are in [Keys](docs/keys.md).

## Documentation

- [Installing](docs/installing.md): prebuilt binaries, Nix, and building
  from source.
- [Configuration](docs/configuration.md): every setting, the keymap, key
  notation and the actions.
- [Keys](docs/keys.md): modes, the default keymap, commands, and where the
  editors differ from vim.
- [Terminal setup](docs/terminal.md): tmux, the clipboard, images and input
  methods.
- [Files](docs/files.md): where mxt keeps its data, the logs, and what
  happens when the session ends.
- [Changelog](CHANGELOG.md): the changes in each release.
- [Design](docs/superpowers/specs/2026-11-18-mxt-design.md): how mxt is
  built, for contributors.
- [Releasing](docs/releasing.md): versions and how a release is cut, for
  maintainers.

## Development

`nix develop` provides the toolchain (cargo, rustc, clippy, rustfmt,
rust-analyzer, cargo-deny and prek). CI runs `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test` and
`cargo deny check licenses advisories sources` on Linux and macOS.

## License

MIT; see [LICENSE](LICENSE).

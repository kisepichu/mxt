# mxt

mxt is a terminal [Matrix](https://matrix.org) client with nvim-like
keybindings. It is made for accounts where most rooms are bridged from other
networks such as Discord or Slack: it shows which network a room belongs to,
makes unread rooms easy to reach, and lets you rebind every key. End-to-end
encryption, sync and unread counts come from
[matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk); mxt is the
user interface around it. It runs on Linux and macOS.

## Features

- Password login, end-to-end encryption, and the recovery key on the first
  start, so the new device is cross-signed and reads older messages from the
  key backup.
- A room list that names bridged rooms `<network> / <room>` (for example
  `Discord / Alice`) and shows one unread badge per room: mentions,
  notifications, unread messages (in muted rooms too) or the manual unread
  mark.
- A fuzzy room picker, the same picker for rooms with unread mentions, and an
  action that opens the next unread room.
- A timeline with formatted messages, replies, edits, reactions and read
  receipts, and a full-screen view for long messages and images.
- A composer with vim modes, Markdown, `@` mention completion, replies and
  edits, and an external editor for long text.
- Reactions from an emoji picker, editing and deleting your own messages, and
  resending messages that failed.
- Muting rooms, downloading and opening attachments, uploading files, and
  image previews in sixel, kitty or iTerm2 graphics, or in half blocks.
- A keymap in the configuration file: every key can be rebound, and a key can
  run any action or a sequence of actions.

Not included: threads, desktop and typing notifications, SSO login, logout,
several accounts in one process, and setting up cross-signing or recovery for
an account that has none (do that in another client first).

## Installing

### With Nix

```sh
nix profile install github:kisepichu/mxt
```

The flake builds `packages.default` for `x86_64-linux`, `aarch64-linux` and
`aarch64-darwin`. To install it from a NixOS or home-manager configuration,
add the flake as an input:

```nix
{
  inputs.mxt.url = "github:kisepichu/mxt";
  # ...
}
```

and add the package in a module that gets the flake inputs (for example
through `specialArgs`):

```nix
{ pkgs, inputs, ... }:
{
  # home-manager: home.packages
  environment.systemPackages = [
    inputs.mxt.packages.${pkgs.stdenv.hostPlatform.system}.default
  ];
}
```

### Without Nix

Requirements:

- Rust 1.96 or newer.
- SQLite 3.54.0 or newer, with its development files. mxt links the system
  SQLite instead of bundling one, and older versions have a known
  vulnerability (RUSTSEC-2026-0459). `pkg-config --modversion sqlite3` shows
  the version that the build will find.
- `pkg-config`, which the build uses to find SQLite.
- A C compiler: some dependencies build C code.

```sh
cargo install --locked --git https://github.com/kisepichu/mxt mxt
```

`--locked` builds with the dependency versions in the repository's
`Cargo.lock`. If the system SQLite is too old, install a newer one and point
`pkg-config` at it, for example with Homebrew on macOS:

```sh
PKG_CONFIG_PATH="$(brew --prefix sqlite)/lib/pkgconfig" \
  cargo install --locked --git https://github.com/kisepichu/mxt mxt
```

## First start

1. Run `mxt`. Without a configuration file it prints where the file belongs
   and a minimal example, and exits:

   ```text
   mxt: /home/alice/.config/mxt/config.toml does not exist. A minimal configuration:

   [account]
   user_id = "@alice:example.org"
   ```

   The file is `$XDG_CONFIG_HOME/mxt/config.toml`, or
   `~/.config/mxt/config.toml` when `XDG_CONFIG_HOME` is not set, on macOS
   too. `mxt --config PATH` reads another file.

2. Create the file with your user ID. mxt finds your homeserver from the
   server name in the user ID (`.well-known`); if that does not work, set
   `homeserver` in `[account]`. Everything else has a default; see
   [Configuration](docs/configuration.md).

3. Run `mxt` again. It logs in as a new device and asks for your password
   without echoing it:

   ```text
   Logging in as @alice:example.org on https://matrix.example.org/
   Password:
   ```

   The session is saved, and later starts restore it without asking.

4. mxt starts syncing, which uploads the new device's keys, waits up to 90
   seconds for those keys to reach the server, and then asks for your
   recovery key (some clients call it the security key), again without echo:

   ```text
   Waiting for this device's keys to reach the server...
   Recovery key (empty to skip):
   ```

   With the recovery key, mxt cross-signs the device and fetches room keys
   from the key backup when a message cannot be decrypted, so you can read
   older encrypted messages, and bridges that accept only cross-signed
   devices accept this one. If the account has no secret storage, mxt says
   so and continues; set up recovery in another client.

   An empty input skips recovery, and a failed recovery prints the reason
   and continues. mxt then says that `:recover` asks again later, but in this
   version `:recover` only answers "account.recover is not available yet".
   To enter the key later, log in again as a new device: quit mxt, delete
   `session.json` in the data directory (see [Files](#files)) and start mxt.
   The old device stays in your account's device list; remove it from
   another client.

5. The terminal UI starts. `:q` quits. The keys are in [Keys](docs/keys.md).

Only one mxt can use a data directory at a time, because two processes must
never share the encryption store. A second one exits with
`mxt: another mxt is running (lock: <data directory>/lock)`.

## Files

| Directory | Default | Contents |
|---|---|---|
| data, `$XDG_DATA_HOME/mxt` | `~/.local/share/mxt` | `session.json` (the homeserver, user ID, device ID and access token), `lock`, and `store/`, the SDK's state and encryption stores |
| cache, `$XDG_CACHE_HOME/mxt` | `~/.cache/mxt` | the SDK's event cache and media cache, which hold decrypted messages and media |
| state, `$XDG_STATE_HOME/mxt` | `~/.local/state/mxt` | the logs |

The defaults apply on macOS too, and also when a variable is empty or a
relative path. `HOME` must be an absolute path. mxt creates the directories
with mode 0700 and tightens existing ones to 0700; `session.json` and `lock`
have mode 0600. The stores have no passphrase: the permissions protect them.
The cache directory must not contain the data or state directory, because
mxt deletes the cache directory when it sets data aside (below). Downloads
go to `files.download_dir`, not to the cache.

Logs are written to daily files `mxt.<date>.log` (dates in UTC), and the
last seven are kept; nothing is logged to the terminal. `MXT_LOG` takes a
[tracing `EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
directive, for example `MXT_LOG=debug` or `MXT_LOG=warn,mxt_matrix=debug`.
The default is `warn,mxt=info,mxt_core=info,mxt_matrix=info,mxt_tui=info`.

When the session cannot be used any more, mxt moves its data aside, prints
why, and exits; the next start logs in as a new device, and asks for the
recovery key again:

- The homeserver answers `M_UNKNOWN_TOKEN`, for example after the device was
  signed out from another client: mxt restores the terminal, deletes the
  cache directory, and renames the data directory to
  `mxt.invalid-<unix seconds>` next to it.
- `session.json` exists but `store/` does not: a new store would give the
  existing device new encryption keys, so mxt does the same. The old device
  stays in the account's device list.

`session.json` belongs to one account. If `account.user_id` names another
one, mxt stops and names both. Deleting `session.json` switches accounts: the
next start moves the old `store/` to `store.invalid-<unix seconds>` in the
data directory, empties the cache directory and logs in. The homeserver in
`session.json` is used even when `account.homeserver` changes.

## Documentation

- [Configuration](docs/configuration.md): every setting, the keymap, key
  notation and the actions.
- [Keys](docs/keys.md): modes, the default keymap, commands, and where the
  editors differ from vim.
- [Terminal setup](docs/terminal.md): tmux, the clipboard, images and input
  methods.
- [Design](docs/superpowers/specs/2026-11-18-mxt-design.md): how mxt is
  built, for contributors.

## Development

`nix develop` provides the toolchain (cargo, rustc, clippy, rustfmt,
rust-analyzer, cargo-deny and prek). CI runs `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test` and
`cargo deny check licenses advisories sources` on Linux and macOS.

## License

MIT; see [LICENSE](LICENSE).

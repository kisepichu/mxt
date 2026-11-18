# Phase 0 (spike) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide the flake development shell, and answer the seven phase 0
questions of the design (sync path, bridge state, unread counts,
`/notifications`, edtui and CJK, remote mentions, key backup) with throwaway
programs, then record the answers in the design.

**Architecture:** The flake is permanent and lands on `main`. The two spike
programs are throwaway: each lives on its own branch (`spike/matrix`,
`spike/edtui-cjk`) as an extra workspace member under `spike/`, is published
as a draft pull request for visibility, and is never merged. Their findings go
into the design document in a separate pull request, and the branches are
deleted afterwards. This keeps matrix-sdk out of `main`'s `Cargo.lock` until
`mxt-matrix` needs it.

**Tech Stack:** Nix flakes (nixos-unstable), Rust 2024, matrix-sdk and
matrix-sdk-ui 0.19, tokio, edtui 0.11, ratatui 0.30, crossterm 0.29.

## Global Constraints

- The design is `docs/superpowers/specs/2026-11-18-mxt-design.md`. It wins over
  this plan when they disagree.
- Everything in the repository is in English. No host names, real user IDs,
  room IDs, tokens or logs are committed. Tests and examples use
  `@alice:example.org` and `!room:example.org`.
- Access tokens, passwords and recovery keys are never printed or logged.
  Message bodies only at `trace` level (the spike prints bodies to its own
  terminal on request, which is not logging).
- `rust-version = "1.96"`; matrix-sdk 0.19 needs Rust 1.96 or later.
- Every cargo invocation runs inside `nix develop` once the flake exists
  (`nix develop -c cargo ...`), so all builds use the flake's toolchain.
- Dependencies are declared once in the root `[workspace.dependencies]` and
  used with `<name>.workspace = true`.

## File Structure

| Path | Branch | Responsibility |
|---|---|---|
| `flake.nix`, `flake.lock` | `main` | dev shell, package, checks |
| `spike/mxt-spike/` | `spike/matrix` | throwaway SDK spike binary |
| `spike/edtui-cjk/` | `spike/edtui-cjk` | throwaway edtui CJK check program and its tmux check script |
| `docs/superpowers/specs/2026-11-18-mxt-design.md` | `docs/phase0-findings` | findings |

---

### Task 1: The flake (development shell, package, checks)

**Files:**
- Create: `flake.nix`, `flake.lock`

**Interfaces:**
- Produces: `nix develop` with cargo, rustc, clippy, rustfmt, rust-analyzer,
  cargo-deny and prek from the pinned nixpkgs; `packages.<system>.default`
  (the `mxt` binary); `checks.<system>.package`.

- [ ] **Step 1: Write `flake.nix`**

```nix
{
  description = "mxt, a terminal Matrix client with nvim-like keybindings";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      # nixpkgs dropped x86_64-darwin in 2026.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      workspace = (lib.importTOML ./Cargo.toml).workspace.package;
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "mxt";
          inherit (workspace) version;
          # Only what Cargo needs, so editing docs does not rebuild the package.
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--package" "mxt" ];
          cargoTestFlags = [ "--workspace" ];
          # matrix-sdk needs these (the same inputs as nixpkgs' iamb package).
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [
            pkgs.sqlite
            pkgs.aws-lc
          ];
          meta = {
            description = "A terminal Matrix client with nvim-like keybindings";
            homepage = "https://github.com/kisepichu/mxt";
            license = lib.licenses.mit;
            mainProgram = "mxt";
          };
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            cargo-deny
            prek
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      checks = forAllSystems (pkgs: {
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      });
    };
}
```

`lib.fileset.unions` must list `./crates` only while every crate lives there.
When the `spike/` branches add members, they do not touch `flake.nix` (they are
never built by Nix).

- [ ] **Step 2: Lock and check the toolchain version**

Run: `nix flake lock && nix develop -c rustc --version`
Expected: `rustc 1.96.0` or later. If older, stop and report; the design
requires 1.96.

- [ ] **Step 3: Build the shell tools and the package**

Run: `nix develop -c sh -c 'cargo --version && cargo deny --version && prek --version && cargo build'`
Then: `nix build -L .#` and `nix flake check -L`
Expected: all succeed; `./result/bin/mxt` exists. The workspace has no
third-party dependencies yet, so these are quick.

- [ ] **Step 4: Commit**

```bash
git add flake.nix flake.lock
git commit -m "Add the flake so every machine and CI build with the same pinned toolchain, and so mxt can be installed with Nix"
```

---

### Task 2: The matrix spike binary (`spike/matrix` branch)

Runs against a real account. Read the safety rules first; they are part of the
deliverable, enforced in code.

**Safety rules (enforced by the program):**
- Writing commands (`send`) refuse any room except the one recorded in
  `<data dir>/test-room` by `create-test-room`, unless `--allow-bridged` is
  given. Only a human passes `--allow-bridged`.
- The spike never sends read receipts, never sets or clears the fully read
  marker, never marks rooms unread, never changes notification settings,
  never joins, leaves or invites.
- `login` refuses to run when `<data dir>/session.json` exists (each login
  creates a new device).
- Passwords and recovery keys are read without echo and never printed. The
  session file has mode 0600 and the data directory 0700. The access token is
  never printed.
- Logging goes to stderr with `warn` as the default level; nothing at `info`
  or below prints event contents.

**Files:**
- Modify: `Cargo.toml` (add `"spike/*"` to `members`; add the dependencies
  below to `[workspace.dependencies]`)
- Create: `spike/mxt-spike/Cargo.toml`, `spike/mxt-spike/src/main.rs`, and
  further modules under `spike/mxt-spike/src/` as needed (one per command
  keeps them readable)

**Dependencies:** `matrix-sdk = { version = "0.19", default-features = false,
features = ["e2e-encryption", "sqlite", "rustls-tls"] }` (check the 0.19
feature names on docs.rs; add `"automatic-room-key-forwarding"` only if needed),
`matrix-sdk-ui = { version = "0.19", default-features = false, features = [...] }`
(the features needed for `SyncService` and `Timeline` with e2e encryption),
`tokio` (`rt-multi-thread`, `macros`, `time`, `signal`), `serde`, `serde_json`,
`rpassword`, `futures-util`, `anyhow`, `tracing`, `tracing-subscriber`
(`env-filter`). Keep `default-features = false` where the default pulls in
native TLS or unused features.

**Command line:** `mxt-spike [--data-dir PATH] <command> [args]`. The data
directory defaults to `$XDG_DATA_HOME/mxt-spike` (or `~/.local/share/mxt-spike`).
Use `std::env::args` parsing; no clap needed.

| Command | Phase 0 question | Behavior |
|---|---|---|
| `login <user_id>` | — | Discover the homeserver from the user ID's server name (`ClientBuilder::server_name_or_homeserver_url`), log in with the password (no refresh token, device display name `mxt-spike`), save the session to `session.json` (0600). If the server does not offer `m.login.password`, print the login types it offers and exit non-zero. Then ask for the recovery key without echo; empty input skips. Call `Recovery::recover`. If the account has no secret storage (`RecoveryState::Disabled`), print that recovery has to be set up in another client. Run sync until the device keys are uploaded and print: recovery state, whether cross-signing keys are available, and whether this device is cross-signed by its owner. |
| `rooms [--wait SECS] [--json PATH]` | (1), (2), (3) | Restore the session, start `SyncService` (simplified sliding sync, discovered natively), wait until the `RoomListService` all-rooms list reports `Loaded` and holds `maximum_number_of_rooms` entries (or until `--wait`, default 120 s). Fetch bridge state with the one-off MSC4186 requests (below). For every room print: network name by the design's order and which step produced it, the room part of the name (other user's display name for DMs), DM or group, `num_unread_messages`, `num_unread_notifications`, `num_unread_mentions`, `is_marked_unread`, the user-defined notification mode, and the latest activity time. Then print totals: sliding sync version in use, time until the list was loaded, number of rooms, rooms with `m.bridge`, with `uk.half-shot.bridge`, named from spaces, without a network name. `--json` writes the same data as JSON (this file is local output; never commit it). |
| `bridge-raw <room_id>` | (2) | Print the raw `m.bridge` and `uk.half-shot.bridge` state of one room from the one-off request, for inspecting which fields a bridge fills. |
| `room <room_id> [--back N] [--wait SECS]` | (3), (7) | Subscribe a `Timeline` for the room, paginate backwards until `N` events (default 50) are loaded, and print one line per item: time, sender, kind, whether it is undecryptable, and the first 60 characters of the plain body. Report how many items are undecryptable right after loading and again after `--wait` seconds (default 30), to show whether `AfterDecryptionFailure` fetched keys from backup. Then report the SDK's `num_unread_messages` next to the real count: the number of message events from other users after the user's latest own receipt (the later of `m.read` and `m.read.private`, read with `Room::load_user_receipt`), paginating back until the receipt's event is found or 500 events were loaded. |
| `members <room_id> [--filter TEXT]` | (6) | Print the user ID and display name of joined members, filtered case-insensitively by `TEXT`. Lets the owner find a bridged user's Matrix ID for the mention check. |
| `notifications [--limit N]` | (4) | Send `GET /_matrix/client/v3/notifications` (ruma `push::get_notifications::v3`) with `limit` (default 20). Print whether it succeeded, the HTTP error otherwise, and per notification: time, room, whether it is a highlight. No bodies. |
| `classic-probe` | (1) | Send one classic `/sync` request with `Client::send` (not `Client::sync_once`, so nothing reaches the SDK store) with a filter of timeline limit 1 and lazy-loaded members, `timeout` 0, and report status, response time, response size and number of joined rooms. This shows whether classic sync is a usable fallback. |
| `create-test-room` | — | If `<data dir>/test-room` exists, print its room ID and exit. Otherwise create a private room named `mxt-test` (preset `private_chat`, not in the directory, no invites) with `m.room.encryption` (`m.megolm.v1.aes-sha2`) in the initial state, write its ID to `<data dir>/test-room`, and print it. |
| `send <room_id> <text> [--mention USER_ID=TEXT] [--allow-bridged]` | (6) | Refuse unless `room_id` equals the content of `<data dir>/test-room` or `--allow-bridged` is given. Send a text message: the plain body is `<text>` unchanged. When `--mention` is given and `TEXT` occurs in `<text>`, the HTML body is `<text>` HTML-escaped with the first occurrence of `TEXT` replaced by `<a href="https://matrix.to/#/USER_ID">TEXT</a>`, and `m.mentions.user_ids` is `[USER_ID]` (`RoomMessageEventContent::text_html(..).add_mentions(Mentions::with_user_ids(..))`). Wait until the local echo is sent and print the event ID. |

**Client settings (all commands):** sqlite store in `<data dir>/store` with no
passphrase; `EncryptionSettings { backup_download_strategy:
BackupDownloadStrategy::AfterDecryptionFailure, auto_enable_backups: false,
auto_enable_cross_signing: false, .. }`; no refresh token handling.

**One-off bridge state request** (design, "Matrix adapter"): build
`ruma::api::client::sync::sync_events::v5::Request` (via `matrix_sdk::ruma`),
with one list named `bridges`, `ranges` `[0..=99]`, `timeline_limit` 0,
`required_state` `[("m.bridge", "*"), ("uk.half-shot.bridge", "*")]`, no
extensions, and send it with `Client::send`. Read the list's `count`; request
the next ranges (`100..=199`, ...) until `count` is covered. Try fresh requests
without `pos` first; if the server rejects ranges past the first page without
a `pos`, chain `pos` from the previous response, and write down which worked.
Parse the `required_state` of every returned room yourself (serde_json on the
raw events): `content.network.displayname` and `content.protocol.displayname`.
The responses must never be passed to the SDK's sync processing.

**Network name order** (design): (1) `network.displayname` of `m.bridge` or
`uk.half-shot.bridge`, (2) `protocol.displayname`, (3) the name of the room's
canonical parent space, (4) the name of the parent space when the user has
joined exactly one, (5) none. Parent spaces come from the room's
`m.space.parent` state (`Room::parent_spaces()` in the SDK); a space's name is
its display name, available only for joined spaces.

- [ ] **Step 1: Create the branch and the crate**

```bash
git switch -c spike/matrix origin/main
```

Add `"spike/*"` to the root `members`, the dependencies to
`[workspace.dependencies]`, and `spike/mxt-spike/Cargo.toml` with
`publish = false` and `[lints] workspace = true`.

- [ ] **Step 2: Implement `login`, session restore and the client builder**

Look up the exact 0.19 APIs in the downloaded sources
(`~/.cargo/registry/src/*/matrix-sdk-0.19.*`) rather than guessing:
`ClientBuilder::sqlite_store`, `ClientBuilder::with_encryption_settings`,
`ClientBuilder::sliding_sync_version_builder`,
`Client::matrix_auth().login_username(..)`, the session type to serialize, and
`Client::restore_session`.

- [ ] **Step 3: Build**

Run (one matrix-sdk build at a time on this machine, see the worker rules):
`nix develop -c cargo build -p mxt-spike`
Expected: success. Fix native dependency problems in the flake's
`buildInputs` only if the flake on `main` lacks something, and report that
change separately (it belongs on `main`).

- [ ] **Step 4: Implement the remaining commands**, building after each.

- [ ] **Step 5: Check the commands that work without an account**

Run: `./target/debug/mxt-spike rooms` with a temporary empty `--data-dir`.
Expected: a clear "no session; run login first" error and a non-zero exit.
Run: `./target/debug/mxt-spike send '!room:example.org' hi` with that data
dir. Expected: refused before any network access.

- [ ] **Step 6: Commit and open a draft pull request** titled "Phase 0 matrix
spike (not for merging)" that lists the commands and the questions each one
answers.

---

### Task 3: The edtui CJK check (`spike/edtui-cjk` branch)

**Files:**
- Modify: `Cargo.toml` (`"spike/*"` in `members`; `edtui`, `ratatui`,
  `crossterm`, `serde_json`, `unicode-width` in `[workspace.dependencies]`)
- Create: `spike/edtui-cjk/Cargo.toml`, `spike/edtui-cjk/src/main.rs`,
  `spike/edtui-cjk/check.sh`

**Program:** `edtui-cjk --out PATH`. A full-screen ratatui program: an edtui
`EditorView` in vim mode filling the screen except the last line, edtui with
`default-features = false, features = ["system-editor"]` as the design
specifies. The last line shows edtui's mode, the cursor as (row, char column),
and the expected display column (the sum of `unicode-width` widths of the
characters before the cursor on its line). After every handled key the program
writes `{"mode": .., "lines": [..], "row": .., "col": .., "display_col": ..}` to
`PATH`. `<C-q>` quits and restores the terminal.

**Check script:** `check.sh` builds the program, starts it in a new tmux window
of the current session (`tmux new-window -d -n cjk-check ...`; never use a
session named `0`), and drives it with `tmux send-keys`. `send-keys` with
Japanese text stands in for committed input method text; preedit display stays
a human check. After each scenario it reads `PATH` and compares, and compares
`#{cursor_x}` and `#{cursor_y}` from `tmux display -p -t <pane>` with where
the cursor must be drawn. It prints PASS or FAIL per scenario with the actual
values, captures the pane (`tmux capture-pane -p`) on failure, and kills its
window at the end.

Scenarios (each starts from an empty buffer):
1. `i`, `こんにちは世界`, `<Esc>`: text is `こんにちは世界`, cursor on `界`
   (char column 6), drawn at display column 12.
2. Scenario 1, then `0`, `l`, `l`: char column 2, display column 4.
3. Scenario 1, then `0`, `x`: text `んにちは世界`.
4. Scenario 1, then `0`, `w`, `e`, `b`: record where each motion lands (vim
   treats a run of kana and kanji as one word; note edtui's behavior, not a
   failure).
5. `i`, `abc日本語def`, `<Esc>`, `F`, `本`: cursor on `本` (char 4, display 5).
6. `i`, `日本語`, `<Left>`, `<Left>`, `X`, `<Esc>`: text `日X本語`.
7. A line of 60 `あ` in a pane narrower than 120 columns: record whether edtui
   wraps, and whether a wide character at the wrap boundary is drawn whole.
8. `i`, `👍🏽 and 👨‍👩‍👧`, `<Esc>`, `0`, `l`, `l`: record where the cursor lands
   (grapheme clusters versus chars).
9. `yy`, `p` on a CJK line: two identical lines (the internal register works
   without arboard).

- [ ] **Step 1: Create the branch, the crate and the program.**
- [ ] **Step 2: Build:** `nix develop -c cargo build -p edtui-cjk`.
- [ ] **Step 3: Run `check.sh`** and keep its full output.
- [ ] **Step 4: Commit and open a draft pull request** titled "Phase 0 edtui
CJK check (not for merging)", with the script output in the description.

---

### Task 4: Record the findings in the design

Runs after the owner has logged in with the spike and after the checks that
need a human (the bridged mention, input method preedit) are done.

**Files:**
- Modify: `docs/superpowers/specs/2026-11-18-mxt-design.md`

- [ ] **Step 1:** Add a section `## Phase 0 findings` before "Risks and open
questions", with one subsection per question (1)–(7): what was run (command
names, not real IDs), what was observed (counts and timings, anonymized), and
the decision.
- [ ] **Step 2:** Apply what the design says phase 0 decides: the sync path
in "Matrix adapter" (keep the paragraph for the chosen path only), the
mentions list in "Mentions" (whether `/notifications` is supported), and the
`room.mark_read` call when found. Update "Risks and open questions" for risks
that are settled.
- [ ] **Step 3:** Commit on branch `docs/phase0-findings`, open a pull
request, and close the spike draft pull requests and delete their branches
after it merges.

# Phase 1 (skeleton) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A room list with network names and unread badges that can be
browsed with configurable keys, built on the crate layout, CI and core
domain of the design.

**Architecture:** Functional core in `mxt-core` (domain types, keymap and
resolver, actions, `App::update`), configuration DTOs in `mxt`, the SDK
adapter in `mxt-matrix`, and the terminal adapter in `mxt-tui`. Tasks that do
not depend on the phase 0 decisions come first; the sync, startup and room
list pane tasks are added to this plan after phase 0 decides the sync path.

**Tech Stack:** Rust 2024, Nix flake toolchain, GitHub Actions, prek,
cargo-deny, nucleo-matcher.

## Global Constraints

- The design is `docs/superpowers/specs/2026-11-18-mxt-design.md`. It wins over
  this plan when they disagree.
- Everything in the repository is in English. Tests use made-up identifiers
  (`@alice:example.org`, `!room:example.org`).
- `mxt-core` depends only on `std` and small pure crates without I/O
  (`nucleo-matcher`, `emojis`, `pulldown-cmark`, `unicode-segmentation`). Never
  the SDK, `ruma`, an async runtime, a terminal library, `serde` or `toml`.
- `mxt-core` performs no I/O and reads no clock. Timestamps are UTC
  milliseconds passed in by the adapters.
- Identifiers in the core are mxt's own newtypes, not `ruma` types.
- No repository traits, dependency injection containers or one type per use
  case.
- Dependencies are declared once in the root `[workspace.dependencies]` and
  used with `<name>.workspace = true`.
- All cargo commands run inside the flake's shell: `nix develop -c cargo ...`.
- Checks before every commit: `nix develop -c cargo fmt --check`,
  `nix develop -c cargo clippy --all-targets -- -D warnings`,
  `nix develop -c cargo test`.

---

### Task 1: CI, prek and cargo-deny

**Files:**
- Create: `.github/workflows/ci.yml`, `.github/workflows/nix.yml`,
  `.pre-commit-config.yaml`, `deny.toml`

**Interfaces:**
- Consumes: the flake from phase 0 Task 1 (`nix develop` provides cargo,
  clippy, rustfmt and cargo-deny; `checks.<system>.package` builds the
  package).

- [ ] **Step 1: Write `deny.toml`**

```toml
# Only `licenses` and `advisories` are checked (see the CI workflow).

[licenses]
version = 2
confidence-threshold = 0.8
# Permissive licenses and MPL-2.0. GPL-family licenses, including the AGPL,
# are rejected by not being listed.
allow = [
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "BSL-1.0",
  "CC0-1.0",
  "CDLA-Permissive-2.0",
  "ISC",
  "MIT",
  "MIT-0",
  "MPL-2.0",
  "Unicode-3.0",
  "Unlicense",
  "Zlib",
]

[licenses.private]
# The workspace crates are not published.
ignore = true

[advisories]
version = 2
yanked = "deny"
```

Mark the workspace crates `publish = false` (in each crate's `Cargo.toml`)
so `[licenses.private]` applies to them.

Run: `nix develop -c cargo deny check licenses advisories`
Expected: `licenses ok`, `advisories ok` (warnings about allowed licenses that
are not encountered are fine).

- [ ] **Step 2: Write `.github/workflows/ci.yml`**

```yaml
name: CI

# Pushes to pull request branches run through `pull_request`, so `push` only
# needs `main`; this avoids running every job twice.
on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  check:
    strategy:
      fail-fast: false
      matrix:
        # macos-latest runs on Apple Silicon.
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v5
      - uses: cachix/install-nix-action@v31
      - name: Enter the development shell
        run: nix develop -c true
      - uses: Swatinem/rust-cache@v2
      - run: nix develop -c cargo fmt --check
      - run: nix develop -c cargo clippy --all-targets -- -D warnings
      - run: nix develop -c cargo test
      - run: nix develop -c cargo deny check licenses advisories
```

Use the newest major versions of the actions that exist at the time of
writing (check their repositories), and pin by major version tag.

- [ ] **Step 3: Write `.github/workflows/nix.yml`**

```yaml
name: Nix package

# The full Nix build compiles matrix-rust-sdk without incremental caching, so
# it runs only when the build inputs change, weekly, and on demand.
on:
  push:
    branches: [main]
    paths: [flake.nix, flake.lock, Cargo.lock, "**/Cargo.toml"]
  pull_request:
    paths: [flake.nix, flake.lock, Cargo.lock, "**/Cargo.toml"]
  schedule:
    - cron: "17 3 * * 1"
  workflow_dispatch:

permissions:
  contents: read

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v5
      - uses: cachix/install-nix-action@v31
      - run: nix flake check -L
```

- [ ] **Step 4: Write `.pre-commit-config.yaml`** (prek reads the
pre-commit format)

```yaml
# Run `prek install` to enable. The hooks expect the flake's shell
# (`nix develop`) or an equivalent toolchain on PATH.
repos:
  - repo: local
    hooks:
      - id: cargo-fmt
        name: cargo fmt --check
        entry: cargo fmt --all --check
        language: system
        types: [rust]
        pass_filenames: false
      - id: cargo-clippy
        name: cargo clippy
        entry: cargo clippy --all-targets -- -D warnings
        language: system
        types: [rust]
        pass_filenames: false
```

Run: `nix develop -c prek run --all-files`
Expected: both hooks pass. Do not run `prek install` in agent worktrees.

- [ ] **Step 5: Commit, push, open the pull request, and confirm both
workflows pass on GitHub** (`gh pr checks --watch`). The `nix.yml` workflow
runs on this pull request only if a path filter matches; trigger it once with
`gh workflow run nix.yml --ref <branch>` after pushing, and confirm it passes.

```bash
git add .github/workflows/ci.yml .github/workflows/nix.yml .pre-commit-config.yaml deny.toml crates/*/Cargo.toml
git commit -m "Add CI, prek hooks and the cargo-deny license policy so formatting, lints, tests and dependency licenses are checked on every change"
```

---

### Task 2: Core identifiers, room summaries, badges, network names and room targets

**Files:**
- Modify: `crates/mxt-core/Cargo.toml` (add `nucleo-matcher`), root
  `Cargo.toml` (`nucleo-matcher = "0.3"` in `[workspace.dependencies]`),
  `crates/mxt-core/src/lib.rs`
- Create: `crates/mxt-core/src/ids.rs`, `crates/mxt-core/src/room.rs`,
  `crates/mxt-core/src/rooms.rs`, `crates/mxt-core/src/fuzzy.rs`
- Test: unit tests in `#[cfg(test)] mod tests` at the bottom of each file

**Interfaces:**
- Produces (used by every later core task and by `mxt-matrix`):

```rust
// ids.rs
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RoomId(String);   // likewise EventId, TxnId, UserId
impl RoomId {
    /// No validation: the adapter converts already-valid ruma identifiers.
    pub fn new(id: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
}
impl std::fmt::Display for RoomId;

/// UTC milliseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(u64);
impl Timestamp {
    pub fn from_millis(ms: u64) -> Self;
    pub fn as_millis(self) -> u64;
}

// room.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomName { pub network: Option<String>, pub room: String }
impl std::fmt::Display for RoomName; // "network / room", or "room"

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomKind { Direct, Group }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NotificationMode { #[default] Default, MentionsAndKeywords, Mute }
impl NotificationMode {
    /// "Muted" for `Mute`, "Mentions only" for `MentionsAndKeywords`, else None.
    pub fn label(self) -> Option<&'static str>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct UnreadCounts { pub messages: u64, pub notifications: u64, pub mentions: u64, pub marked: bool }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomSummary {
    pub id: RoomId,
    pub name: RoomName,
    pub kind: RoomKind,
    pub unread: UnreadCounts,
    pub mode: NotificationMode,
    pub latest_activity: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge { Mentions(u64), Notifications(u64), Messages(u64), Marked }
impl Badge {
    /// First matching rule: mentions > 0, notifications > 0, messages > 0, marked.
    pub fn from_counts(counts: &UnreadCounts) -> Option<Badge>;
    /// "@3", "12", "7", "U"; counts above 99 show as "99+" ("@99+" for mentions).
    pub fn text(self) -> String;
}

/// Display names from a room's `m.bridge` (or `uk.half-shot.bridge`) event.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BridgeInfo { pub network: Option<String>, pub protocol: Option<String> }

/// The design's network name order; the first that yields a non-empty name wins:
/// bridge network, bridge protocol, canonical parent space name, the name of the
/// parent space when the user has joined exactly one, none.
pub fn network_name(
    bridge: Option<&BridgeInfo>,
    canonical_parent: Option<&str>,
    joined_parents: &[&str],
) -> Option<String>;

// rooms.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RoomSort { #[default] Recent, Name }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RoomFilter { #[default] All, Unread, Notify, Mentions }
impl RoomFilter { pub fn matches(self, counts: &UnreadCounts) -> bool; }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pick { #[default] First, Next, Prev }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomTarget {
    Pick { filter: RoomFilter, pick: Pick },
    Fuzzy { filter: RoomFilter, query: String },
}

/// Joined rooms in display order.
#[derive(Clone, Debug, Default)]
pub struct RoomList { /* private: sort, rooms in order */ }
impl RoomList {
    pub fn new(sort: RoomSort) -> Self;
    pub fn sort(&self) -> RoomSort;
    /// Inserts or replaces the room with the same id and restores the order.
    pub fn upsert(&mut self, summary: RoomSummary);
    pub fn remove(&mut self, id: &RoomId) -> Option<RoomSummary>;
    pub fn rooms(&self) -> &[RoomSummary];
    pub fn get(&self, id: &RoomId) -> Option<&RoomSummary>;
    pub fn position(&self, id: &RoomId) -> Option<usize>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    /// Optimistic clearing: sets the room's counts to zero (and `marked` to false).
    /// The order does not depend on counts, so it stays the same.
    pub fn clear_unread(&mut self, id: &RoomId);
}

pub fn resolve_room(rooms: &RoomList, current: Option<&RoomId>, target: &RoomTarget) -> Option<RoomId>;

// fuzzy.rs
/// Ranks `candidates` against `query` with nucleo-matcher (smart case, smart
/// normalization). Returns the indices of matching candidates, best first;
/// equal scores keep the candidates' order. An empty query matches every
/// candidate in order.
pub fn rank<S: AsRef<str>>(query: &str, candidates: &[S]) -> Vec<usize>;
```

**Rules to implement:**
- `RoomName` display: `"{network} / {room}"` when `network` is `Some`,
  otherwise `room`.
- Network names: a `Some("")` or whitespace-only value counts as missing at
  every step. Step 4 applies only when `joined_parents.len() == 1`.
- `RoomFilter::matches`: `All` always; `Unread`: messages, notifications or
  mentions above zero, or marked; `Notify`: notifications or mentions above
  zero, or marked; `Mentions`: mentions above zero.
- Order `Recent`: `latest_activity` newest first, rooms without activity last;
  ties by the `Name` order. Order `Name`: case-insensitive (`to_lowercase`) by
  the full displayed name `network / room`; ties by room id. The order is total
  and deterministic.
- `resolve_room`:
  - `Pick { pick: First }`: the first room in order that passes the filter.
  - `Pick { pick: Next }`: the first room passing the filter after `current`
    in order, wrapping around, never `current` itself; `None` if no other room
    passes. When `current` is `None` or not in the list: like `First`.
  - `Pick { pick: Prev }`: the same, backwards; when `current` is `None` or not
    in the list: the last room in order that passes the filter.
  - `Fuzzy { query }`: among rooms passing the filter, the best
    `fuzzy::rank` match on the displayed name; `None` if nothing matches.

- [ ] **Step 1: Write the failing tests**

`ids.rs` needs no tests beyond what the types make obvious. In `room.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn counts(messages: u64, notifications: u64, mentions: u64, marked: bool) -> UnreadCounts {
        UnreadCounts { messages, notifications, mentions, marked }
    }

    #[test]
    fn badge_prefers_mentions_then_notifications_then_messages_then_marked() {
        assert_eq!(Badge::from_counts(&counts(9, 5, 2, true)), Some(Badge::Mentions(2)));
        assert_eq!(Badge::from_counts(&counts(9, 5, 0, true)), Some(Badge::Notifications(5)));
        assert_eq!(Badge::from_counts(&counts(9, 0, 0, true)), Some(Badge::Messages(9)));
        assert_eq!(Badge::from_counts(&counts(0, 0, 0, true)), Some(Badge::Marked));
        assert_eq!(Badge::from_counts(&counts(0, 0, 0, false)), None);
    }

    #[test]
    fn badge_text_caps_at_99() {
        assert_eq!(Badge::Mentions(3).text(), "@3");
        assert_eq!(Badge::Mentions(100).text(), "@99+");
        assert_eq!(Badge::Notifications(99).text(), "99");
        assert_eq!(Badge::Messages(250).text(), "99+");
        assert_eq!(Badge::Marked.text(), "U");
    }

    #[test]
    fn mode_labels() {
        assert_eq!(NotificationMode::Mute.label(), Some("Muted"));
        assert_eq!(NotificationMode::MentionsAndKeywords.label(), Some("Mentions only"));
        assert_eq!(NotificationMode::Default.label(), None);
    }

    #[test]
    fn room_name_display() {
        let bridged = RoomName { network: Some("myserver".into()), room: "#general".into() };
        assert_eq!(bridged.to_string(), "myserver / #general");
        let plain = RoomName { network: None, room: "Book club".into() };
        assert_eq!(plain.to_string(), "Book club");
    }

    fn bridge(network: Option<&str>, protocol: Option<&str>) -> BridgeInfo {
        BridgeInfo { network: network.map(Into::into), protocol: protocol.map(Into::into) }
    }

    #[test]
    fn network_name_prefers_bridge_network_then_protocol() {
        let b = bridge(Some("myserver"), Some("Discord"));
        assert_eq!(network_name(Some(&b), Some("Space"), &["Space"]).as_deref(), Some("myserver"));
        // A Discord DM: the bridge sets only the protocol.
        let dm = bridge(None, Some("Discord"));
        assert_eq!(network_name(Some(&dm), None, &[]).as_deref(), Some("Discord"));
    }

    #[test]
    fn network_name_treats_empty_values_as_missing() {
        let b = bridge(Some("  "), Some(""));
        assert_eq!(network_name(Some(&b), Some("Space"), &[]).as_deref(), Some("Space"));
    }

    #[test]
    fn network_name_falls_back_to_parent_spaces() {
        assert_eq!(network_name(None, Some("Canonical"), &["Other"]).as_deref(), Some("Canonical"));
        assert_eq!(network_name(None, None, &["Only"]).as_deref(), Some("Only"));
        assert_eq!(network_name(None, None, &["One", "Two"]), None);
        assert_eq!(network_name(None, None, &[]), None);
    }
}
```

In `rooms.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{RoomId, Timestamp};
    use crate::room::{NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts};

    fn room(id: &str, network: Option<&str>, name: &str, at: Option<u64>, unread: UnreadCounts) -> RoomSummary {
        RoomSummary {
            id: RoomId::new(id),
            name: RoomName { network: network.map(Into::into), room: name.into() },
            kind: RoomKind::Group,
            unread,
            mode: NotificationMode::Default,
            latest_activity: at.map(Timestamp::from_millis),
        }
    }

    fn unread(messages: u64) -> UnreadCounts {
        UnreadCounts { messages, ..UnreadCounts::default() }
    }

    fn ids(list: &RoomList) -> Vec<&str> {
        list.rooms().iter().map(|r| r.id.as_str()).collect()
    }

    fn list(sort: RoomSort, rooms: Vec<RoomSummary>) -> RoomList {
        let mut list = RoomList::new(sort);
        for r in rooms {
            list.upsert(r);
        }
        list
    }

    #[test]
    fn recent_order_is_newest_first_with_inactive_rooms_last() {
        let l = list(RoomSort::Recent, vec![
            room("!a:example.org", None, "a", Some(10), unread(0)),
            room("!b:example.org", None, "b", None, unread(0)),
            room("!c:example.org", None, "c", Some(30), unread(0)),
        ]);
        assert_eq!(ids(&l), ["!c:example.org", "!a:example.org", "!b:example.org"]);
    }

    #[test]
    fn name_order_is_case_insensitive_on_the_full_name() {
        let l = list(RoomSort::Name, vec![
            room("!1:example.org", Some("Work"), "#general", Some(1), unread(0)),
            room("!2:example.org", Some("home"), "#random", Some(2), unread(0)),
            room("!3:example.org", None, "Alice", Some(3), unread(0)),
        ]);
        assert_eq!(ids(&l), ["!3:example.org", "!2:example.org", "!1:example.org"]);
    }

    #[test]
    fn upsert_replaces_and_reorders() {
        let mut l = list(RoomSort::Recent, vec![
            room("!a:example.org", None, "a", Some(10), unread(0)),
            room("!b:example.org", None, "b", Some(20), unread(0)),
        ]);
        l.upsert(room("!a:example.org", None, "a", Some(30), unread(1)));
        assert_eq!(ids(&l), ["!a:example.org", "!b:example.org"]);
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn filters() {
        let marked = UnreadCounts { marked: true, ..UnreadCounts::default() };
        let notif = UnreadCounts { messages: 1, notifications: 1, ..UnreadCounts::default() };
        let mention = UnreadCounts { messages: 1, notifications: 1, mentions: 1, marked: false };
        assert!(RoomFilter::All.matches(&UnreadCounts::default()));
        assert!(RoomFilter::Unread.matches(&unread(1)));
        assert!(RoomFilter::Unread.matches(&marked));
        assert!(!RoomFilter::Unread.matches(&UnreadCounts::default()));
        assert!(!RoomFilter::Notify.matches(&unread(1)));
        assert!(RoomFilter::Notify.matches(&notif));
        assert!(RoomFilter::Notify.matches(&marked));
        assert!(!RoomFilter::Mentions.matches(&notif));
        assert!(RoomFilter::Mentions.matches(&mention));
    }

    fn three_unread() -> RoomList {
        // Order: c, b, a (newest first). b is read.
        list(RoomSort::Recent, vec![
            room("!a:example.org", None, "a", Some(10), unread(1)),
            room("!b:example.org", None, "b", Some(20), unread(0)),
            room("!c:example.org", None, "c", Some(30), unread(2)),
        ])
    }

    fn pick(filter: RoomFilter, pick: Pick) -> RoomTarget {
        RoomTarget::Pick { filter, pick }
    }

    #[test]
    fn pick_first_and_no_current() {
        let l = three_unread();
        let id = |s: &str| Some(RoomId::new(s));
        assert_eq!(resolve_room(&l, None, &pick(RoomFilter::Unread, Pick::First)), id("!c:example.org"));
        assert_eq!(resolve_room(&l, None, &pick(RoomFilter::Unread, Pick::Next)), id("!c:example.org"));
        assert_eq!(resolve_room(&l, None, &pick(RoomFilter::Unread, Pick::Prev)), id("!a:example.org"));
    }

    #[test]
    fn pick_next_wraps_around_and_skips_current() {
        let l = three_unread();
        let a = RoomId::new("!a:example.org");
        let c = RoomId::new("!c:example.org");
        assert_eq!(resolve_room(&l, Some(&c), &pick(RoomFilter::Unread, Pick::Next)), Some(a.clone()));
        assert_eq!(resolve_room(&l, Some(&a), &pick(RoomFilter::Unread, Pick::Next)), Some(c.clone()));
        assert_eq!(resolve_room(&l, Some(&c), &pick(RoomFilter::Unread, Pick::Prev)), Some(a.clone()));
    }

    #[test]
    fn pick_next_from_a_read_current_room() {
        let l = three_unread();
        let b = RoomId::new("!b:example.org");
        assert_eq!(
            resolve_room(&l, Some(&b), &pick(RoomFilter::Unread, Pick::Next)),
            Some(RoomId::new("!a:example.org"))
        );
    }

    #[test]
    fn pick_next_returns_none_when_only_current_matches() {
        let l = list(RoomSort::Recent, vec![
            room("!a:example.org", None, "a", Some(10), unread(1)),
            room("!b:example.org", None, "b", Some(20), unread(0)),
        ]);
        let a = RoomId::new("!a:example.org");
        assert_eq!(resolve_room(&l, Some(&a), &pick(RoomFilter::Unread, Pick::Next)), None);
    }

    #[test]
    fn clear_unread_makes_the_next_pick_skip_the_room() {
        let mut l = three_unread();
        let c = RoomId::new("!c:example.org");
        l.clear_unread(&c);
        assert_eq!(l.get(&c).unwrap().unread, UnreadCounts::default());
        assert_eq!(
            resolve_room(&l, None, &pick(RoomFilter::Unread, Pick::First)),
            Some(RoomId::new("!a:example.org"))
        );
    }

    #[test]
    fn fuzzy_target_matches_the_displayed_name_within_the_filter() {
        let l = list(RoomSort::Recent, vec![
            room("!1:example.org", Some("work"), "#general", Some(10), unread(0)),
            room("!2:example.org", Some("home"), "#general", Some(20), unread(3)),
        ]);
        let all = RoomTarget::Fuzzy { filter: RoomFilter::All, query: "work gen".into() };
        assert_eq!(resolve_room(&l, None, &all), Some(RoomId::new("!1:example.org")));
        let unread_only = RoomTarget::Fuzzy { filter: RoomFilter::Unread, query: "general".into() };
        assert_eq!(resolve_room(&l, None, &unread_only), Some(RoomId::new("!2:example.org")));
        let none = RoomTarget::Fuzzy { filter: RoomFilter::All, query: "zzzz".into() };
        assert_eq!(resolve_room(&l, None, &none), None);
    }
}
```

In `fuzzy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_keeps_every_candidate_in_order() {
        assert_eq!(rank("", &["b", "a"]), vec![0, 1]);
    }

    #[test]
    fn best_match_first_and_non_matches_dropped() {
        let names = ["home / #random", "work / #general", "home / #general"];
        let ranked = rank("homegen", &names);
        assert_eq!(ranked.first(), Some(&2));
        assert!(!ranked.contains(&1));
    }

    #[test]
    fn smart_case() {
        assert_eq!(rank("alice", &["Alice"]), vec![0]);
        assert!(rank("Alice", &["alice"]).is_empty());
    }

    #[test]
    fn matches_japanese_names() {
        assert_eq!(rank("雑談", &["work / #general", "home / 雑談"]), vec![1]);
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo test -p mxt-core`
Expected: compile errors for the missing items.

- [ ] **Step 3: Implement** `ids.rs`, `room.rs`, `rooms.rs`, `fuzzy.rs` and
declare the modules in `lib.rs` (`pub mod ids; pub mod room; pub mod rooms;
pub mod fuzzy;`). Use `nucleo_matcher::pattern::{Pattern, CaseMatching,
Normalization}` and one `nucleo_matcher::Matcher` per call to `rank`
(`Matcher::new(Config::DEFAULT)`); `Pattern::parse(query, CaseMatching::Smart,
Normalization::Smart)` treats spaces as separate atoms, which is what
`"work gen"` relies on.

- [ ] **Step 4: Run the tests to see them pass**

Run: `nix develop -c cargo test -p mxt-core`
Expected: all pass. Then the clippy and fmt checks from the global constraints.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/mxt-core
git commit -m "Add the core room domain: identifiers, summaries, badges, network names and room targets, so filters and ordering are defined once in the core"
```

---

### Task 3: Key notation

**Files:**
- Create: `crates/mxt-core/src/keys.rs`
- Modify: `crates/mxt-core/src/lib.rs` (`pub mod keys;`)

**Interfaces:**
- Produces (used by the keymap, the resolver, the configuration loader and
  the tui adapter, which converts crossterm events with `Key::new`):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeyCode {
    Char(char), Enter, Esc, Tab, Backspace, Delete, Insert,
    Up, Down, Left, Right, Home, End, PageUp, PageDown, F(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Modifiers { pub ctrl: bool, pub alt: bool, pub shift: bool }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key { code: KeyCode, mods: Modifiers }   // fields private: always normalized
impl Key {
    /// Normalizes like nvim: for `Char`, shift is folded into the character
    /// (`a` + shift becomes `A`) and dropped; with ctrl, ASCII letters are
    /// lowercased (`<C-A>` is `<C-a>`).
    pub fn new(code: KeyCode, mods: Modifiers) -> Key;
    /// A plain character without modifiers.
    pub fn char(c: char) -> Key;
    pub fn code(self) -> KeyCode;
    pub fn mods(self) -> Modifiers;
    /// `Some(c)` for a `Char` without ctrl or alt: text a text input should insert.
    pub fn text(self) -> Option<char>;
}
/// Notation form: `a`, `A`, `<Space>`, `<lt>`, `<CR>`, `<C-x>`, `<A-x>`, `<S-Tab>`, `<C-A-x>`, `<F5>`.
impl std::fmt::Display for Key;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct KeySeq(Vec<Key>);
impl KeySeq {
    /// Parses nvim key notation; `<leader>` expands to `leader`.
    pub fn parse(text: &str, leader: &KeySeq) -> Result<KeySeq, KeyParseError>;
    pub fn from_keys(keys: Vec<Key>) -> KeySeq;
    pub fn keys(&self) -> &[Key];
    pub fn is_empty(&self) -> bool;
}
impl std::fmt::Display for KeySeq;   // concatenated `Key` notation

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyParseError {
    Empty,
    /// A `<` without a matching `>`. Write `<lt>` for a literal `<`.
    Unclosed { at: usize },
    UnknownKey { name: String },
    UnknownModifier { name: String },
    /// `<leader>` where no leader is available (inside the leader itself).
    NoLeader,
}
impl std::fmt::Display for KeyParseError;
impl std::error::Error for KeyParseError {}
```

**Notation rules:**
- Any character outside `<...>` is that character (`a`, `:`, `]`, `+`, `/`,
  `日`). A `<` must start a `<...>` group.
- Names inside `<...>` are case-insensitive: `CR`, `Enter` and `Return`
  (Enter); `Esc`; `Tab`; `BS` and `Backspace`; `Del` and `Delete`; `Ins` and
  `Insert`; `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`,
  `PageDown`; `F1` to `F12`; `Space` (`Char(' ')`); `lt` (`Char('<')`);
  `Bslash` (`Char('\\')`); `Bar` (`Char('|')`); `leader`.
- Modifier prefixes inside `<...>`, case-insensitive, in any order and
  combination: `C-` (ctrl), `A-` and `M-` (alt), `S-` (shift). The part after
  the modifiers is one character or a name: `<C-x>`, `<C-->`, `<S-Tab>`,
  `<C-A-x>`, `<M-CR>`. `<leader>` takes no modifiers.
- The result is normalized with `Key::new`: `<S-a>` equals `A`, `<C-A>` equals
  `<C-a>`.
- Leader: `KeySeq::parse(leader_text, &KeySeq::default())`; an empty leader
  makes `<leader>` fail with `NoLeader`.

- [ ] **Step 1: Write the failing tests** (in `keys.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(c: char) -> Key {
        Key::new(KeyCode::Char(c), Modifiers { ctrl: true, ..Modifiers::default() })
    }
    fn code(code: KeyCode) -> Key {
        Key::new(code, Modifiers::default())
    }
    fn parse(text: &str) -> Result<KeySeq, KeyParseError> {
        KeySeq::parse(text, &KeySeq::from_keys(vec![Key::char(' ')]))
    }
    fn keys(text: &str) -> Vec<Key> {
        parse(text).unwrap().keys().to_vec()
    }

    #[test]
    fn plain_characters() {
        assert_eq!(keys("gg"), vec![Key::char('g'), Key::char('g')]);
        assert_eq!(keys("]u"), vec![Key::char(']'), Key::char('u')]);
        assert_eq!(keys("日本"), vec![Key::char('日'), Key::char('本')]);
    }

    #[test]
    fn special_names_are_case_insensitive() {
        assert_eq!(keys("<CR>"), vec![code(KeyCode::Enter)]);
        assert_eq!(keys("<cr>"), vec![code(KeyCode::Enter)]);
        assert_eq!(keys("<Enter><Return>"), vec![code(KeyCode::Enter), code(KeyCode::Enter)]);
        assert_eq!(keys("<Esc><Tab><BS><Del>"), vec![
            code(KeyCode::Esc), code(KeyCode::Tab), code(KeyCode::Backspace), code(KeyCode::Delete),
        ]);
        assert_eq!(keys("<F12>"), vec![code(KeyCode::F(12))]);
        assert_eq!(keys("<Space><lt><Bslash><Bar>"), vec![
            Key::char(' '), Key::char('<'), Key::char('\\'), Key::char('|'),
        ]);
    }

    #[test]
    fn modifiers() {
        assert_eq!(keys("<C-x><C-e>"), vec![ctrl('x'), ctrl('e')]);
        assert_eq!(keys("<C-X>"), vec![ctrl('x')]);
        assert_eq!(keys("<c-w>h"), vec![ctrl('w'), Key::char('h')]);
        assert_eq!(keys("<C-->"), vec![ctrl('-')]);
        let alt_x = Key::new(KeyCode::Char('x'), Modifiers { alt: true, ..Modifiers::default() });
        assert_eq!(keys("<A-x>"), vec![alt_x]);
        assert_eq!(keys("<M-x>"), vec![alt_x]);
        let ctrl_alt = Key::new(KeyCode::Char('x'), Modifiers { ctrl: true, alt: true, shift: false });
        assert_eq!(keys("<C-A-x>"), vec![ctrl_alt]);
        assert_eq!(keys("<A-C-x>"), vec![ctrl_alt]);
        let shift_tab = Key::new(KeyCode::Tab, Modifiers { shift: true, ..Modifiers::default() });
        assert_eq!(keys("<S-Tab>"), vec![shift_tab]);
    }

    #[test]
    fn shift_folds_into_characters() {
        assert_eq!(keys("<S-a>"), vec![Key::char('A')]);
        let shifted = Key::new(KeyCode::Char('A'), Modifiers { shift: true, ..Modifiers::default() });
        assert_eq!(shifted, Key::char('A'));
    }

    #[test]
    fn leader_expands() {
        let comma = KeySeq::parse(",", &KeySeq::default()).unwrap();
        let seq = KeySeq::parse("<leader><leader>e", &comma).unwrap();
        assert_eq!(seq.keys(), &[Key::char(','), Key::char(','), Key::char('e')]);
        assert_eq!(keys("<Leader>m"), vec![Key::char(' '), Key::char('m')]);
        assert_eq!(KeySeq::parse("<leader>", &KeySeq::default()), Err(KeyParseError::NoLeader));
    }

    #[test]
    fn errors() {
        assert_eq!(parse(""), Err(KeyParseError::Empty));
        assert_eq!(parse("a<C-x"), Err(KeyParseError::Unclosed { at: 1 }));
        assert_eq!(parse("<"), Err(KeyParseError::Unclosed { at: 0 }));
        assert!(matches!(parse("<Foo>"), Err(KeyParseError::UnknownKey { .. })));
        assert!(matches!(parse("<X-a>"), Err(KeyParseError::UnknownModifier { .. })));
        assert!(matches!(parse("<C-leader>"), Err(KeyParseError::UnknownKey { .. })));
    }

    #[test]
    fn display_round_trips() {
        for text in ["gg", "<C-x><C-e>", "<Space>e", "<lt>", "<CR>", "<S-Tab>", "<C-A-x>", "<F5>", "]u"] {
            let seq = parse(text).unwrap();
            assert_eq!(parse(&seq.to_string()).unwrap(), seq, "{text}");
        }
        assert_eq!(parse("<c-X>").unwrap().to_string(), "<C-x>");
    }

    #[test]
    fn text_is_only_unmodified_characters() {
        assert_eq!(Key::char('a').text(), Some('a'));
        assert_eq!(Key::char('日').text(), Some('日'));
        assert_eq!(ctrl('a').text(), None);
        assert_eq!(code(KeyCode::Enter).text(), None);
    }
}
```

- [ ] **Step 2: Run them to see them fail:** `nix develop -c cargo test -p mxt-core keys`
- [ ] **Step 3: Implement `keys.rs`.** The parser walks the string by
`char_indices`; `Unclosed { at }` is the byte offset of the `<`.
- [ ] **Step 4: Run the tests to see them pass**, then fmt and clippy.
- [ ] **Step 5: Commit**

```bash
git add crates/mxt-core/src/keys.rs crates/mxt-core/src/lib.rs
git commit -m "Add nvim key notation parsing so keymaps and the configuration share one normalized key type"
```

---

### Task 4: Actions, bindings and commands

**Files:**
- Create: `crates/mxt-core/src/action.rs`, `crates/mxt-core/src/command.rs`
- Modify: `crates/mxt-core/src/lib.rs` (`pub mod action; pub mod command;`)

**Interfaces:**
- Consumes: `RoomFilter`, `Pick`, `RoomTarget` (Task 2).
- Produces:

```rust
// action.rs
/// A neutral argument value; the configuration loader converts TOML values into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgValue { String(String), Integer(i64), Boolean(bool) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FocusMode { #[default] Insert, Keep }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    RoomOpen(RoomTarget), RoomOpenSelected, RoomMarkRead, RoomMute, RoomUnmute,
    MessageReply, MessageEdit, MessageDelete, MessageYank, MessageReact,
    MessageOpen, MessageGotoReply, MessageResend, MessageView,
    ComposerFocus(FocusMode), ComposerSend, ComposerNewline, ComposerCancel,
    ComposerExternalEditor,
    CompletionNext, CompletionPrev, CompletionAccept, CompletionCancel,
    PickerRooms(RoomFilter), PickerNext, PickerPrev, PickerAccept, PickerCancel,
    CommandLineOpen, CommandLineExecute, CommandLineCancel, CommandLineComplete,
    CursorDown, CursorUp, CursorFirst, CursorLast, CursorHalfPageDown, CursorHalfPageUp,
    SearchStart, SearchNext, SearchPrev,
    ViewClose, SidebarToggle, FocusRoomList, FocusTimeline,
    FileUpload { path: String }, AccountRecover, AppQuit, Nop,
}
impl Action {
    /// Builds a checked action from its name and arguments (see the table below).
    pub fn from_spec(name: &str, args: &[(String, ArgValue)]) -> Result<Action, ActionError>;
    /// The registry name, for example "room.open".
    pub fn name(&self) -> &'static str;
    /// Movement actions that take a count prefix: cursor.down, cursor.up,
    /// cursor.half_page_down, cursor.half_page_up.
    pub fn takes_count(&self) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    UnknownAction { name: String },
    UnknownArgument { action: &'static str, arg: String },
    WrongType { action: &'static str, arg: &'static str, expected: &'static str },
    InvalidValue { action: &'static str, arg: &'static str, value: String, allowed: &'static [&'static str] },
    Conflict { action: &'static str, args: [&'static str; 2] },
    MissingArgument { action: &'static str, arg: &'static str },
}
impl std::fmt::Display for ActionError;   // for example: `room.open: "pick" and "query" cannot be used together`
impl std::error::Error for ActionError {}

/// One or more actions run in order. Never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding(Vec<Action>);
impl Binding {
    /// `None` for an empty list.
    pub fn new(actions: Vec<Action>) -> Option<Binding>;
    pub fn single(action: Action) -> Binding;
    pub fn actions(&self) -> &[Action];
}

// command.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    Unknown { name: String },
    MissingArgument { command: &'static str, usage: &'static str },
    UnexpectedArgument { command: &'static str },
}
impl std::fmt::Display for CommandError;
impl std::error::Error for CommandError {}

/// Command names, in the order completion offers them.
pub const COMMANDS: &[&str] = &["open", "mute", "unmute", "upload", "recover", "q"];
/// Whether the command takes an argument (completion appends a space after it).
pub fn takes_argument(command: &str) -> bool;

/// Parses a command line without the leading `:`. `Ok(None)` for a blank line.
/// Everything after the command name, trimmed, is one argument.
pub fn parse_command(line: &str) -> Result<Option<Action>, CommandError>;
```

**Action arguments** (every other action takes none; any argument given to
them is `UnknownArgument`):

| Action | Argument | Type | Values | Default |
|---|---|---|---|---|
| `room.open` | `filter` | string | `all`, `unread`, `notify`, `mentions` | `all` |
| `room.open` | `pick` | string | `first`, `next`, `prev` | `first` |
| `room.open` | `query` | string | any | none |
| `picker.rooms` | `filter` | string | as above | `all` |
| `composer.focus` | `mode` | string | `insert`, `keep` | `insert` |
| `file.upload` | `path` | string | any non-empty | required |

`room.open` with both `pick` and `query` is `Conflict`. With `query` it is
`RoomTarget::Fuzzy { filter, query }`, otherwise `RoomTarget::Pick { filter,
pick }`.

**Commands:**

| Command | Action | Argument |
|---|---|---|
| `open <room>` | `RoomOpen(Fuzzy { filter: All, query })` | required |
| `mute`, `unmute` | `RoomMute`, `RoomUnmute` | none |
| `upload <path>` | `FileUpload { path }` | required |
| `recover` | `AccountRecover` | none |
| `q` | `AppQuit` | none |

- [ ] **Step 1: Write the failing tests**

```rust
// action.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::{Pick, RoomFilter, RoomTarget};

    fn s(k: &str, v: &str) -> (String, ArgValue) {
        (k.to_owned(), ArgValue::String(v.to_owned()))
    }

    #[test]
    fn actions_without_arguments() {
        assert_eq!(Action::from_spec("cursor.down", &[]), Ok(Action::CursorDown));
        assert_eq!(Action::from_spec("nop", &[]), Ok(Action::Nop));
        assert_eq!(Action::from_spec("app.quit", &[]), Ok(Action::AppQuit));
    }

    #[test]
    fn every_action_name_round_trips() {
        let names = [
            "room.open", "room.open_selected", "room.mark_read", "room.mute", "room.unmute",
            "message.reply", "message.edit", "message.delete", "message.yank", "message.react",
            "message.open", "message.goto_reply", "message.resend", "message.view",
            "composer.focus", "composer.send", "composer.newline", "composer.cancel",
            "composer.external_editor", "completion.next", "completion.prev",
            "completion.accept", "completion.cancel", "picker.rooms", "picker.next",
            "picker.prev", "picker.accept", "picker.cancel", "command_line.open",
            "command_line.execute", "command_line.cancel", "command_line.complete",
            "cursor.down", "cursor.up", "cursor.first", "cursor.last",
            "cursor.half_page_down", "cursor.half_page_up", "search.start", "search.next",
            "search.prev", "view.close", "sidebar.toggle", "focus.room_list",
            "focus.timeline", "account.recover", "app.quit", "nop",
        ];
        for name in names {
            let action = Action::from_spec(name, &[]).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(action.name(), name);
        }
        let upload = Action::from_spec("file.upload", &[s("path", "/tmp/a b.png")]).unwrap();
        assert_eq!(upload.name(), "file.upload");
    }

    #[test]
    fn room_open_arguments() {
        assert_eq!(
            Action::from_spec("room.open", &[s("filter", "unread"), s("pick", "next")]),
            Ok(Action::RoomOpen(RoomTarget::Pick { filter: RoomFilter::Unread, pick: Pick::Next }))
        );
        assert_eq!(
            Action::from_spec("room.open", &[]),
            Ok(Action::RoomOpen(RoomTarget::Pick { filter: RoomFilter::All, pick: Pick::First }))
        );
        assert_eq!(
            Action::from_spec("room.open", &[s("query", "general")]),
            Ok(Action::RoomOpen(RoomTarget::Fuzzy { filter: RoomFilter::All, query: "general".into() }))
        );
    }

    #[test]
    fn pick_and_query_conflict() {
        assert_eq!(
            Action::from_spec("room.open", &[s("pick", "next"), s("query", "x")]),
            Err(ActionError::Conflict { action: "room.open", args: ["pick", "query"] })
        );
    }

    #[test]
    fn rejects_unknown_actions_and_wrong_arguments() {
        assert_eq!(
            Action::from_spec("room.opne", &[]),
            Err(ActionError::UnknownAction { name: "room.opne".into() })
        );
        assert_eq!(
            Action::from_spec("cursor.down", &[s("filter", "all")]),
            Err(ActionError::UnknownArgument { action: "cursor.down", arg: "filter".into() })
        );
        assert_eq!(
            Action::from_spec("room.open", &[("pick".into(), ArgValue::Integer(1))]),
            Err(ActionError::WrongType { action: "room.open", arg: "pick", expected: "string" })
        );
        assert!(matches!(
            Action::from_spec("room.open", &[s("filter", "unreads")]),
            Err(ActionError::InvalidValue { action: "room.open", arg: "filter", .. })
        ));
        assert_eq!(
            Action::from_spec("file.upload", &[]),
            Err(ActionError::MissingArgument { action: "file.upload", arg: "path" })
        );
    }

    #[test]
    fn composer_focus_mode() {
        assert_eq!(Action::from_spec("composer.focus", &[]), Ok(Action::ComposerFocus(FocusMode::Insert)));
        assert_eq!(
            Action::from_spec("composer.focus", &[s("mode", "keep")]),
            Ok(Action::ComposerFocus(FocusMode::Keep))
        );
    }

    #[test]
    fn picker_filter() {
        assert_eq!(
            Action::from_spec("picker.rooms", &[s("filter", "mentions")]),
            Ok(Action::PickerRooms(RoomFilter::Mentions))
        );
    }

    #[test]
    fn counts_apply_to_line_movement_only() {
        assert!(Action::CursorDown.takes_count());
        assert!(Action::CursorHalfPageUp.takes_count());
        assert!(!Action::CursorFirst.takes_count());
        assert!(!Action::RoomMarkRead.takes_count());
    }

    #[test]
    fn empty_binding_is_rejected() {
        assert_eq!(Binding::new(vec![]), None);
        assert_eq!(Binding::new(vec![Action::Nop]).unwrap().actions(), &[Action::Nop]);
    }
}

// command.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::rooms::{RoomFilter, RoomTarget};

    #[test]
    fn open_takes_the_rest_of_the_line() {
        assert_eq!(
            parse_command("open  my room  "),
            Ok(Some(Action::RoomOpen(RoomTarget::Fuzzy { filter: RoomFilter::All, query: "my room".into() })))
        );
    }

    #[test]
    fn upload_keeps_spaces_in_paths() {
        assert_eq!(
            parse_command("upload /tmp/a b.png"),
            Ok(Some(Action::FileUpload { path: "/tmp/a b.png".into() }))
        );
    }

    #[test]
    fn simple_commands() {
        assert_eq!(parse_command("q"), Ok(Some(Action::AppQuit)));
        assert_eq!(parse_command(" mute "), Ok(Some(Action::RoomMute)));
        assert_eq!(parse_command("unmute"), Ok(Some(Action::RoomUnmute)));
        assert_eq!(parse_command("recover"), Ok(Some(Action::AccountRecover)));
        assert_eq!(parse_command("   "), Ok(None));
    }

    #[test]
    fn errors() {
        assert_eq!(parse_command("quit"), Err(CommandError::Unknown { name: "quit".into() }));
        assert!(matches!(parse_command("open"), Err(CommandError::MissingArgument { command: "open", .. })));
        assert_eq!(parse_command("mute now"), Err(CommandError::UnexpectedArgument { command: "mute" }));
    }

    #[test]
    fn argument_flags() {
        assert!(takes_argument("open"));
        assert!(takes_argument("upload"));
        assert!(!takes_argument("q"));
    }
}
```

- [ ] **Step 2: Run them to see them fail:** `nix develop -c cargo test -p mxt-core action command`
- [ ] **Step 3: Implement** `action.rs` and `command.rs`. Keep the name table
in one place (a `match` in `from_spec` and one in `name`, or one static
table); argument parsing is a small helper per argument kind.
- [ ] **Step 4: Run the tests to see them pass**, then fmt and clippy.
- [ ] **Step 5: Commit**

```bash
git add crates/mxt-core/src/action.rs crates/mxt-core/src/command.rs crates/mxt-core/src/lib.rs
git commit -m "Add the action registry and command parsing so keys and commands resolve to the same checked actions"
```

---

### Task 5: Keymap and resolver

**Files:**
- Create: `crates/mxt-core/src/keymap.rs`, `crates/mxt-core/src/resolver.rs`
- Modify: `crates/mxt-core/src/lib.rs`

**Interfaces:**
- Consumes: `Key`, `KeySeq` (Task 3); `Action`, `Binding` (Task 4).
- Produces:

```rust
// keymap.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TableId {
    Normal, RoomList, Timeline, ComposerNormal, ComposerVisual, ComposerInsert,
    ComposerCompletion, MessageView, Picker, CommandLine,
}
impl TableId {
    pub const ALL: [TableId; 10];
    /// "normal", "room_list", "timeline", "composer.normal", "composer.visual",
    /// "composer.insert", "composer.completion", "message_view", "picker", "command_line".
    pub fn config_name(self) -> &'static str;
    pub fn from_config_name(name: &str) -> Option<TableId>;
}

/// Where the user is typing; decides the table chain and where released keys go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputContext {
    RoomList, Timeline,
    ComposerNormal, ComposerVisual, ComposerInsert, ComposerCompletion,
    /// The message view's editor in normal or visual mode.
    MessageView,
    /// An editor in a mode where every key goes to it (search mode; the message
    /// view's editor in insert mode).
    EditorPassthrough,
    Picker, CommandLine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySink {
    /// Panes without text input: released keys are dropped.
    Drop,
    /// The composer's or the message view's editor: `Effect::Ui(ReplayKeys)`.
    Editor,
    /// The picker query or the command line text, owned by the core.
    Text,
}

impl InputContext {
    /// Tables consulted, earliest wins (design, "Tables and resolution").
    pub fn chain(self) -> &'static [TableId];
    pub fn sink(self) -> KeySink;
    /// Whether digits start a count prefix: RoomList and Timeline only.
    pub fn counts(self) -> bool;
}

/// All tables after the user's bindings were merged over the defaults, and one
/// precomputed effective map per input context.
#[derive(Clone, Debug)]
pub struct Keymap { /* private */ }
impl Keymap {
    /// The default keymap with `leader` substituted, overlaid key by key by
    /// `user`: a user binding replaces the default binding of the same table and
    /// key sequence; `nop` bindings stay in the map and shadow later tables.
    pub fn new(leader: &KeySeq, user: impl IntoIterator<Item = (TableId, KeySeq, Binding)>) -> Keymap;
    pub fn effective(&self, context: InputContext) -> &EffectiveMap;
}

#[derive(Clone, Debug, Default)]
pub struct EffectiveMap { /* private: BTreeMap<KeySeq, Binding> */ }

#[derive(Debug, PartialEq, Eq)]
pub struct Lookup<'a> {
    /// The binding for exactly these keys.
    pub binding: Option<&'a Binding>,
    /// Whether a longer binding starts with these keys.
    pub is_prefix: bool,
}

impl EffectiveMap {
    /// Merges the tables in order; for the same key sequence the earlier table wins.
    pub fn merge<'a>(tables: impl IntoIterator<Item = &'a BTreeMap<KeySeq, Binding>>) -> EffectiveMap;
    pub fn lookup(&self, keys: &[Key]) -> Lookup<'_>;
}

// resolver.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimerId(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// Run the binding; `count` is the count prefix if one was typed.
    Run { binding: Binding, count: Option<u32> },
    /// Keys the keymap did not consume, for the context's sink.
    Release(Vec<Key>),
    /// Wait for more keys and ask the tui adapter for a timer.
    StartTimer { id: TimerId, after_ms: u64 },
}

#[derive(Clone, Debug, Default)]
pub struct Resolver { /* private: pending keys, count, current timer, next timer id */ }
impl Resolver {
    pub fn new() -> Resolver;
    pub fn feed(&mut self, map: &EffectiveMap, counts: bool, timeoutlen_ms: u64, key: Key) -> Vec<Resolved>;
    /// Stale or unknown ids return nothing.
    pub fn timeout(&mut self, map: &EffectiveMap, timeoutlen_ms: u64, id: TimerId) -> Vec<Resolved>;
    /// Drops pending keys and the count (when the input context changes).
    pub fn reset(&mut self);
    /// Pending keys, for the status line.
    pub fn pending(&self) -> &[Key];
    pub fn count(&self) -> Option<u32>;
}
```

**Table chains and sinks:**

| Context | Chain | Sink | Counts |
|---|---|---|---|
| `RoomList` | `RoomList`, `Normal` | `Drop` | yes |
| `Timeline` | `Timeline`, `Normal` | `Drop` | yes |
| `ComposerNormal` | `ComposerNormal`, `Normal` | `Editor` | no |
| `ComposerVisual` | `ComposerVisual`, `Normal` | `Editor` | no |
| `ComposerInsert` | `ComposerInsert` | `Editor` | no |
| `ComposerCompletion` | `ComposerCompletion`, `ComposerInsert` | `Editor` | no |
| `MessageView` | `MessageView` | `Editor` | no |
| `EditorPassthrough` | none | `Editor` | no |
| `Picker` | `Picker` | `Text` | no |
| `CommandLine` | `CommandLine` | `Text` | no |

**Default keymap** (design, "Default keymap"):

| Table | Keys | Binding |
|---|---|---|
| `normal` | `:` | `command_line.open` |
| `normal` | `<leader><leader>` | `picker.rooms` |
| `normal` | `<leader>e` | `sidebar.toggle` |
| `normal` | `<leader>m` | `picker.rooms` with `filter = mentions` |
| `normal` | `<C-w>h`, `<C-w>l` | `focus.room_list`, `focus.timeline` |
| `room_list`, `timeline` | `j`, `k` | `cursor.down`, `cursor.up` |
| `room_list`, `timeline` | `gg`, `G` | `cursor.first`, `cursor.last` |
| `room_list`, `timeline` | `<C-d>`, `<C-u>` | `cursor.half_page_down`, `cursor.half_page_up` |
| `room_list`, `timeline` | `/`, `n`, `N` | `search.start`, `search.next`, `search.prev` |
| `room_list` | `<CR>` | `room.open_selected` |
| `timeline` | `i`, `a` | `composer.focus` |
| `timeline` | `<CR>` | `message.view` |
| `timeline` | `r`, `e`, `dd`, `yy`, `+`, `gx`, `gd` | `message.reply`, `message.edit`, `message.delete`, `message.yank`, `message.react`, `message.open`, `message.goto_reply` |
| `composer.insert` | `<CR>`, `<C-j>`, `<C-x><C-e>` | `composer.send`, `composer.newline`, `composer.external_editor` |
| `composer.normal` | `<C-x><C-e>`, `<Esc>`, `<C-c>` | `composer.external_editor`, `focus.timeline`, `composer.cancel` |
| `composer.completion` | `<C-n>`, `<C-p>` | `completion.next`, `completion.prev` |
| `composer.completion` | `<C-y>`, `<Tab>`, `<CR>` | `completion.accept` |
| `composer.completion` | `<C-e>` | `completion.cancel` |
| `picker` | `<C-n>`, `<C-p>`, `<CR>`, `<Esc>` | `picker.next`, `picker.prev`, `picker.accept`, `picker.cancel` |
| `command_line` | `<CR>`, `<Esc>`, `<Tab>` | `command_line.execute`, `command_line.cancel`, `command_line.complete` |
| `message_view` | `q`, `<Esc>` | `view.close` |

Write the defaults as a table of `(TableId, &str notation, Binding)` parsed
with the leader in `Keymap::new`; a default that fails to parse is a bug
(`expect` with the notation in the message), covered by a test.

**Resolution algorithm** (design, "Keymap resolver"):

`feed(key)`:
1. If `counts`, nothing is pending, `key` is an unmodified digit (`1`–`9`, or
   `0` once a count has started) and no binding in `map` starts with that key:
   extend the count (saturating at 9999) and return nothing. No timer runs
   for a count alone.
2. Push `key` to the pending keys and look the sequence up.
   - A longer binding starts with it (`is_prefix`): return
     `StartTimer { id: <fresh id>, after_ms: timeoutlen_ms }`. Every new key
     gets a fresh id, so older timers become stale. This applies whether or not
     the sequence is itself a binding.
   - It is a binding and nothing longer starts with it: return
     `Run { binding, count }` and clear the pending keys, the count and the
     timer.
   - Neither: let `prev` be the pending keys without `key`.
     - If some prefix of `prev` is a binding, take the longest one: return
       `Run` for it (with the count), then feed the rest of `prev` followed by
       `key` again, one by one, as fresh input (count cleared).
     - Otherwise return `Release(prev)` if `prev` is not empty, clear the
       count, and feed `key` again as fresh input. A single key that matches
       nothing with nothing pending is `Release(vec![key])`.

`timeout(id)`: if `id` is the current timer, take the longest prefix of the
pending keys that is a binding; `Run` it and feed the rest again as fresh
input; if there is none, `Release` all pending keys. Clear the count and the
timer either way.

- [ ] **Step 1: Write the failing tests**

```rust
// keymap.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, Binding};
    use crate::keys::{Key, KeySeq};

    fn space() -> KeySeq {
        KeySeq::parse("<Space>", &KeySeq::default()).unwrap()
    }
    fn seq(text: &str) -> KeySeq {
        KeySeq::parse(text, &space()).unwrap()
    }
    fn one(action: Action) -> Binding {
        Binding::single(action)
    }

    #[test]
    fn defaults_parse_and_resolve() {
        let km = Keymap::new(&space(), []);
        let tl = km.effective(InputContext::Timeline);
        assert_eq!(tl.lookup(seq("j").keys()).binding, Some(&one(Action::CursorDown)));
        assert_eq!(tl.lookup(seq(":").keys()).binding, Some(&one(Action::CommandLineOpen)));
        assert_eq!(tl.lookup(seq("<Space><Space>").keys()).binding.map(|b| b.actions()[0].name()), Some("picker.rooms"));
        let g = tl.lookup(seq("g").keys());
        assert_eq!((g.binding, g.is_prefix), (None, true));
    }

    #[test]
    fn insert_mode_does_not_see_normal_or_movement_keys() {
        let km = Keymap::new(&space(), []);
        let ins = km.effective(InputContext::ComposerInsert);
        assert_eq!(ins.lookup(seq("j").keys()).binding, None);
        assert_eq!(ins.lookup(seq(":").keys()).binding, None);
        assert_eq!(ins.lookup(seq("<CR>").keys()).binding, Some(&one(Action::ComposerSend)));
        let normal = km.effective(InputContext::ComposerNormal);
        assert_eq!(normal.lookup(seq(":").keys()).binding, Some(&one(Action::CommandLineOpen)));
        assert_eq!(normal.lookup(seq("j").keys()).binding, None);
    }

    #[test]
    fn completion_falls_back_to_insert() {
        let km = Keymap::new(&space(), []);
        let c = km.effective(InputContext::ComposerCompletion);
        assert_eq!(c.lookup(seq("<CR>").keys()).binding, Some(&one(Action::CompletionAccept)));
        assert_eq!(c.lookup(seq("<C-j>").keys()).binding, Some(&one(Action::ComposerNewline)));
    }

    #[test]
    fn prefixes_are_judged_on_the_merged_map() {
        let km = Keymap::new(&space(), [
            (TableId::Timeline, seq("]"), one(Action::CursorLast)),
            (TableId::Normal, seq("]u"), one(Action::RoomMarkRead)),
        ]);
        let l = km.effective(InputContext::Timeline).lookup(seq("]").keys());
        assert_eq!(l.binding, Some(&one(Action::CursorLast)));
        assert!(l.is_prefix);
    }

    #[test]
    fn earlier_table_wins_and_user_overrides_defaults() {
        let km = Keymap::new(&space(), [
            (TableId::Normal, seq("j"), one(Action::AppQuit)),
            (TableId::Timeline, seq("<C-w>h"), one(Action::SidebarToggle)),
        ]);
        let tl = km.effective(InputContext::Timeline);
        assert_eq!(tl.lookup(seq("j").keys()).binding, Some(&one(Action::CursorDown)));
        assert_eq!(tl.lookup(seq("<C-w>h").keys()).binding, Some(&one(Action::SidebarToggle)));
        let rl = km.effective(InputContext::RoomList);
        assert_eq!(rl.lookup(seq("<C-w>h").keys()).binding, Some(&one(Action::FocusRoomList)));
    }

    #[test]
    fn nop_shadows_the_default() {
        let km = Keymap::new(&space(), [
            (TableId::Timeline, seq("dd"), one(Action::Nop)),
            (TableId::Timeline, seq("D"), one(Action::MessageDelete)),
        ]);
        let tl = km.effective(InputContext::Timeline);
        assert_eq!(tl.lookup(seq("dd").keys()).binding, Some(&one(Action::Nop)));
        assert_eq!(tl.lookup(seq("D").keys()).binding, Some(&one(Action::MessageDelete)));
    }

    #[test]
    fn leader_is_substituted_in_defaults() {
        let comma = KeySeq::parse(",", &KeySeq::default()).unwrap();
        let km = Keymap::new(&comma, []);
        let tl = km.effective(InputContext::Timeline);
        assert_eq!(tl.lookup(&[Key::char(','), Key::char('e')]).binding, Some(&one(Action::SidebarToggle)));
        assert_eq!(tl.lookup(seq("<Space>e").keys()).binding, None);
    }

    #[test]
    fn chains_and_sinks() {
        assert_eq!(InputContext::EditorPassthrough.chain(), &[] as &[TableId]);
        assert_eq!(InputContext::Picker.sink(), KeySink::Text);
        assert_eq!(InputContext::RoomList.sink(), KeySink::Drop);
        assert!(InputContext::Timeline.counts());
        assert!(!InputContext::ComposerNormal.counts());
        for t in TableId::ALL {
            assert_eq!(TableId::from_config_name(t.config_name()), Some(t));
        }
    }
}

// resolver.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, Binding};
    use crate::keymap::{InputContext, Keymap, TableId};
    use crate::keys::{Key, KeySeq};

    const T: u64 = 1000;

    fn space() -> KeySeq {
        KeySeq::parse("<Space>", &KeySeq::default()).unwrap()
    }
    fn seq(text: &str) -> KeySeq {
        KeySeq::parse(text, &space()).unwrap()
    }
    fn k(text: &str) -> Key {
        seq(text).keys()[0]
    }
    fn run(action: Action, count: Option<u32>) -> Resolved {
        Resolved::Run { binding: Binding::single(action), count }
    }
    fn release(text: &str) -> Resolved {
        Resolved::Release(seq(text).keys().to_vec())
    }
    /// Feeds keys and returns all outputs, with timer ids erased for comparison.
    fn feed_all(r: &mut Resolver, km: &Keymap, ctx: InputContext, text: &str) -> Vec<Resolved> {
        seq(text)
            .keys()
            .iter()
            .flat_map(|&key| r.feed(km.effective(ctx), ctx.counts(), T, key))
            .map(erase)
            .collect()
    }
    fn erase(r: Resolved) -> Resolved {
        match r {
            Resolved::StartTimer { after_ms, .. } => Resolved::StartTimer { id: TimerId(0), after_ms },
            other => other,
        }
    }
    fn timer() -> Resolved {
        Resolved::StartTimer { id: TimerId(0), after_ms: T }
    }
    fn last_timer(outputs: &[Resolved]) -> TimerId {
        outputs.iter().rev().find_map(|o| match o {
            Resolved::StartTimer { id, .. } => Some(*id),
            _ => None,
        }).expect("a timer")
    }
    fn defaults() -> Keymap {
        Keymap::new(&space(), [])
    }

    #[test]
    fn complete_binding_runs_immediately() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(feed_all(&mut r, &km, InputContext::Timeline, "j"), vec![run(Action::CursorDown, None)]);
        assert!(r.pending().is_empty());
    }

    #[test]
    fn prefix_waits_then_completes() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(feed_all(&mut r, &km, InputContext::Timeline, "g"), vec![timer()]);
        assert_eq!(r.pending(), seq("g").keys());
        assert_eq!(feed_all(&mut r, &km, InputContext::Timeline, "g"), vec![run(Action::CursorFirst, None)]);
    }

    #[test]
    fn count_prefix() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(feed_all(&mut r, &km, InputContext::Timeline, "5j"), vec![run(Action::CursorDown, Some(5))]);
        assert_eq!(feed_all(&mut r, &km, InputContext::Timeline, "10k"), vec![run(Action::CursorUp, Some(10))]);
        // `0` without a count is an ordinary (unbound) key.
        assert_eq!(feed_all(&mut r, &km, InputContext::Timeline, "0"), vec![release("0")]);
        assert_eq!(r.count(), None);
    }

    #[test]
    fn no_counts_in_text_contexts() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(feed_all(&mut r, &km, InputContext::ComposerNormal, "5"), vec![release("5")]);
    }

    #[test]
    fn bound_digits_are_not_counts() {
        let km = Keymap::new(&space(), [(TableId::Timeline, seq("1"), Binding::single(Action::FocusRoomList))]);
        let mut r = Resolver::new();
        assert_eq!(feed_all(&mut r, &km, InputContext::Timeline, "1"), vec![run(Action::FocusRoomList, None)]);
    }

    #[test]
    fn timeout_releases_an_incomplete_prefix() {
        let km = defaults();
        let mut r = Resolver::new();
        let out = r.feed(km.effective(InputContext::Timeline), true, T, k("g"));
        let id = last_timer(&out);
        assert_eq!(r.timeout(km.effective(InputContext::Timeline), T, id), vec![release("g")]);
        assert!(r.pending().is_empty());
    }

    #[test]
    fn timeout_runs_a_complete_prefix() {
        let km = Keymap::new(&space(), [
            (TableId::Timeline, seq("]"), Binding::single(Action::CursorLast)),
            (TableId::Normal, seq("]u"), Binding::single(Action::RoomMarkRead)),
        ]);
        let ctx = InputContext::Timeline;
        let mut r = Resolver::new();
        let out = r.feed(km.effective(ctx), true, T, k("]"));
        let id = last_timer(&out);
        assert_eq!(r.timeout(km.effective(ctx), T, id), vec![run(Action::CursorLast, None)]);
    }

    #[test]
    fn stale_timers_are_ignored() {
        let km = defaults();
        let ctx = InputContext::Timeline;
        let mut r = Resolver::new();
        let first = last_timer(&r.feed(km.effective(ctx), true, T, k("g")));
        assert_eq!(feed_all(&mut r, &km, ctx, "g"), vec![run(Action::CursorFirst, None)]);
        assert_eq!(r.timeout(km.effective(ctx), T, first), vec![]);
    }

    #[test]
    fn mismatch_runs_the_pending_complete_binding_then_resolves_the_new_key() {
        let km = Keymap::new(&space(), [
            (TableId::Timeline, seq("]"), Binding::single(Action::CursorLast)),
            (TableId::Normal, seq("]u"), Binding::single(Action::RoomMarkRead)),
        ]);
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "]j"),
            vec![timer(), run(Action::CursorLast, None), run(Action::CursorDown, None)]
        );
    }

    #[test]
    fn mismatch_without_binding_releases_pending_keys() {
        let km = defaults();
        let mut r = Resolver::new();
        // composer.insert has <C-x><C-e>; <C-x> then `a` goes to the editor.
        assert_eq!(
            feed_all(&mut r, &km, InputContext::ComposerInsert, "<C-x>a"),
            vec![timer(), release("<C-x>"), release("a")]
        );
    }

    #[test]
    fn new_key_after_a_release_can_start_a_sequence() {
        let km = defaults();
        let mut r = Resolver::new();
        // `gq` is unbound: release `g`, then `q` is unbound too. `gg` after a
        // released `g` still works.
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "gqgg"),
            vec![timer(), release("g"), release("q"), timer(), run(Action::CursorFirst, None)]
        );
    }

    #[test]
    fn count_is_kept_through_a_pending_sequence() {
        let km = defaults();
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "3gg"),
            vec![timer(), run(Action::CursorFirst, Some(3))]
        );
    }

    #[test]
    fn nop_runs_and_consumes_the_keys() {
        let km = Keymap::new(&space(), [(TableId::Timeline, seq("dd"), Binding::single(Action::Nop))]);
        let mut r = Resolver::new();
        assert_eq!(
            feed_all(&mut r, &km, InputContext::Timeline, "dd"),
            vec![timer(), run(Action::Nop, None)]
        );
    }

    #[test]
    fn reset_drops_pending_keys() {
        let km = defaults();
        let mut r = Resolver::new();
        feed_all(&mut r, &km, InputContext::Timeline, "4g");
        r.reset();
        assert!(r.pending().is_empty());
        assert_eq!(r.count(), None);
    }
}
```

- [ ] **Step 2: Run them to see them fail:** `nix develop -c cargo test -p mxt-core keymap resolver`
- [ ] **Step 3: Implement** `keymap.rs` and `resolver.rs`. `EffectiveMap`
uses a `BTreeMap<KeySeq, Binding>`; `is_prefix` is a range query starting
just after the looked-up sequence (`KeySeq` orders lexicographically by keys,
so every longer sequence with that prefix follows it directly). Feeding the
rest again "as fresh input" is a loop over the keys through the same `feed`
logic, appending outputs in order.
- [ ] **Step 4: Run the tests to see them pass**, then fmt and clippy.
- [ ] **Step 5: Commit**

```bash
git add crates/mxt-core/src/keymap.rs crates/mxt-core/src/resolver.rs crates/mxt-core/src/lib.rs
git commit -m "Add the keymap with its default tables and the deterministic resolver, so fallback chains, nop shadowing, pending keys and counts are defined and tested in the core"
```

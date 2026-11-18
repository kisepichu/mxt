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

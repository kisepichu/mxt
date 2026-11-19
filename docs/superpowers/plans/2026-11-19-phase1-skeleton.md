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
    /// Returns the room's old index (if it was present) and its new index.
    pub fn upsert(&mut self, summary: RoomSummary) -> (Option<usize>, usize);
    /// Returns the index the room had and its summary.
    pub fn remove(&mut self, id: &RoomId) -> Option<(usize, RoomSummary)>;
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
    /// Stale or unknown ids return `None`. As implemented (following the
    /// design): the pending keys run only if they are a binding themselves,
    /// otherwise they are released; nothing is fed again.
    pub fn timeout(&mut self, map: &EffectiveMap, id: TimerId) -> Option<Resolved>;
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

---

### Task 6: App state, messages, effects and room list browsing

**Files:**
- Create: `crates/mxt-core/src/settings.rs`, `crates/mxt-core/src/effect.rs`,
  `crates/mxt-core/src/msg.rs`, `crates/mxt-core/src/app.rs` (state, accessors
  and `update` dispatch), `crates/mxt-core/src/app/rooms.rs` (room list
  actions), `crates/mxt-core/src/app/tests.rs`
- Modify: `crates/mxt-core/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 2–5.
- Produces (the adapters build on these; later phases add variants):

```rust
// settings.rs: settings the core needs, converted from the configuration by `mxt`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ImagePreview { #[default] Auto, Off }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReceiptKind { #[default] Public, Private }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub timeoutlen_ms: u64,        // default 1000
    pub room_sort: RoomSort,       // default Recent
    pub room_list_visible: bool,   // default true
    pub image_preview: ImagePreview,
    pub read_receipts: ReceiptKind,
}
impl Default for Settings;

// effect.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect { Matrix(MatrixEffect), Ui(UiEffect) }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatrixEffect {
    SubscribeTimeline(RoomId),
    UnsubscribeTimeline(RoomId),
    /// `room.mark_read`: a receipt for the room's latest event, and clear its manual unread mark.
    MarkRoomRead(RoomId),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEffect {
    StartTimer { id: TimerId, after_ms: u64 },
    /// Keys for the focused editor (the composer's or the message view's), in order.
    ReplayKeys(Vec<Key>),
    Quit,
}

// msg.rs
/// The mode of the edtui editor that has focus, reported with every key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode { Normal, Visual, Insert, Search }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane { RoomList, Timeline }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SyncState { #[default] Starting, Synced, Offline, Retrying }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// `editor` is `Some` while an editor (composer or message view) has focus.
    Key { key: Key, editor: Option<EditorMode> },
    Timeout { id: TimerId },
    RoomUpserted(RoomSummary),
    RoomRemoved(RoomId),
    SyncState(SyncState),
    /// Sent by the tui adapter after a render changed which items are visible.
    Viewport { pane: Pane, first: usize, last: usize },
    TerminalFocus(bool),
    EffectFailed { error: String },
}

// app.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus { RoomList, Timeline, Composer }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind { Info, Error }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusMessage { pub kind: StatusKind, pub text: String }

pub struct App { /* private */ }
impl App {
    pub fn new(keymap: Keymap, settings: Settings) -> App;
    /// Deterministic and free of I/O.
    pub fn update(&mut self, msg: Msg) -> Vec<Effect>;

    // Read access for rendering.
    pub fn rooms(&self) -> &RoomList;
    /// Index of the room list cursor in `rooms().rooms()`.
    pub fn room_cursor(&self) -> Option<usize>;
    pub fn open_room(&self) -> Option<&RoomId>;
    pub fn focus(&self) -> Focus;
    pub fn sidebar_visible(&self) -> bool;
    pub fn sync_state(&self) -> SyncState;
    pub fn terminal_focused(&self) -> bool;
    pub fn status(&self) -> Option<&StatusMessage>;
    /// Pending keys and count, for the status line (like nvim's showcmd).
    pub fn pending_keys(&self) -> &[Key];
    pub fn pending_count(&self) -> Option<u32>;
    /// "NORMAL", "COMMAND", or for an editor "INSERT", "NORMAL", "VISUAL", "SEARCH".
    pub fn mode_label(&self, editor: Option<EditorMode>) -> &'static str;
}
```

**Behavior:**
- Start: focus on the room list when `room_list_visible`, otherwise the
  timeline; no room open; sync state `Starting`; terminal focused.
- `Msg::Key`: clear the status message; compute the input context
  (command line open → `CommandLine`; focus `RoomList` → `RoomList`; focus
  `Timeline` → `Timeline`; focus `Composer` → by `editor`: `Normal` →
  `ComposerNormal`, `Visual` → `ComposerVisual`, `Insert` → `ComposerInsert`
  (`ComposerCompletion` once the mention menu exists, phase 3), `Search` →
  `EditorPassthrough`, `None` → `ComposerNormal`). If the context differs
  from the one the resolver's pending keys started in, `reset` the resolver.
  Feed the key and handle each `Resolved` in order: `Run` runs the binding's
  actions in order, each against the state the previous one left, passing the
  count to actions that `takes_count` (count 1 otherwise); `Release` goes to
  the context's sink (`Drop`: nothing; `Editor`: `UiEffect::ReplayKeys`,
  consecutive releases merged into one effect; `Text`: the command line,
  Task 7); `StartTimer` becomes `UiEffect::StartTimer`.
- `Msg::Timeout`: `Resolver::timeout` with the effective map of the context
  the pending keys started in; handle the outputs as above.
- `Msg::RoomUpserted`: upsert. Until a cursor action, a search or opening a
  room places the cursor, it stays on whichever room is first (so it stays at
  the top while rooms arrive at startup); after that it follows its room by
  id when the order changes.
- `Msg::RoomRemoved`: remove. A cursor on that room moves to the room now at
  the same index (clamped), or none if the list is empty. If it was the open
  room: no room is open and `UnsubscribeTimeline` is returned.
- `Msg::SyncState`, `Msg::TerminalFocus`, `Msg::Viewport`: store.
- `Msg::EffectFailed { error }`: status message of kind `Error`.
- Actions in this task:
  - `cursor.down` / `cursor.up` (count n): move the cursor of the focused
    pane by n, clamped, no wrapping. `cursor.first` / `cursor.last`.
    `cursor.half_page_down` / `_up` (count n): move by
    n × max(1, visible / 2), where visible = `last - first + 1` of the pane's
    last viewport, or 1 without one. In the timeline these act once the
    timeline exists (phase 2); until then they do nothing there.
  - `room.open_selected`: open the room under the cursor.
  - `room.open(target)`: `resolve_room(rooms, open_room, target)`; no match
    sets the status `No matching room`.
  - Opening a room: if it is already open, only focus the timeline.
    Otherwise return `UnsubscribeTimeline(previous)` (when one was open) then
    `SubscribeTimeline(new)`, set it open, move the room list cursor to it and
    focus the timeline.
  - `room.mark_read`: the target is the room under the cursor when the room
    list has focus, otherwise the open room. Clear its counts optimistically
    (`RoomList::clear_unread`) and return `MarkRoomRead(target)`. Without a
    target, status `No room`.
  - `sidebar.toggle`: flip the visibility; hiding it while it has focus moves
    the focus to the timeline.
  - `focus.room_list`: show the room list if hidden, then focus it.
    `focus.timeline`: focus the timeline.
  - `app.quit`: `UiEffect::Quit`. `nop`: nothing.
  - Every other action sets the status `<action name> is not available yet`
    (kind `Info`); the phase that implements it replaces this. Task 7 adds the
    command line and search actions.

- [ ] **Step 1: Write the failing tests** (`app/tests.rs`, declared with
`#[cfg(test)] mod tests;` in `app.rs`)

```rust
use super::*;
use crate::action::{Action, Binding};
use crate::effect::{Effect, MatrixEffect, UiEffect};
use crate::ids::{RoomId, Timestamp};
use crate::keymap::{Keymap, TableId};
use crate::keys::KeySeq;
use crate::msg::{EditorMode, Msg, Pane};
use crate::room::{NotificationMode, RoomKind, RoomName, RoomSummary, UnreadCounts};
use crate::settings::Settings;

fn space() -> KeySeq {
    KeySeq::parse("<Space>", &KeySeq::default()).unwrap()
}
fn seq(text: &str) -> KeySeq {
    KeySeq::parse(text, &space()).unwrap()
}
fn id(s: &str) -> RoomId {
    RoomId::new(s)
}
fn room(rid: &str, at: u64, messages: u64) -> RoomSummary {
    RoomSummary {
        id: id(rid),
        name: RoomName { network: None, room: rid.trim_start_matches('!').into() },
        kind: RoomKind::Group,
        unread: UnreadCounts { messages, ..UnreadCounts::default() },
        mode: NotificationMode::Default,
        latest_activity: Some(Timestamp::from_millis(at)),
    }
}
fn app_with(user: Vec<(TableId, KeySeq, Binding)>) -> App {
    let mut app = App::new(Keymap::new(&space(), user), Settings::default());
    // Order (newest first): !c, !b, !a. !b is read.
    for r in [room("!a:example.org", 10, 1), room("!b:example.org", 20, 0), room("!c:example.org", 30, 2)] {
        app.update(Msg::RoomUpserted(r));
    }
    app
}
fn app() -> App {
    app_with(vec![])
}
fn keys(app: &mut App, text: &str) -> Vec<Effect> {
    keys_in(app, text, None)
}
fn keys_in(app: &mut App, text: &str, editor: Option<EditorMode>) -> Vec<Effect> {
    seq(text).keys().iter().flat_map(|&key| app.update(Msg::Key { key, editor })).collect()
}
fn cursor_id(app: &App) -> &str {
    app.rooms().rooms()[app.room_cursor().unwrap()].id.as_str()
}
fn no_timers(effects: Vec<Effect>) -> Vec<Effect> {
    effects.into_iter().filter(|e| !matches!(e, Effect::Ui(UiEffect::StartTimer { .. }))).collect()
}

#[test]
fn starts_on_the_room_list_with_the_cursor_on_the_first_room() {
    let app = app();
    assert_eq!(app.focus(), Focus::RoomList);
    assert_eq!(cursor_id(&app), "!c:example.org");
    assert_eq!(app.mode_label(None), "NORMAL");
}

#[test]
fn cursor_moves_with_counts_and_clamps() {
    let mut app = app();
    keys(&mut app, "j");
    assert_eq!(cursor_id(&app), "!b:example.org");
    keys(&mut app, "5j");
    assert_eq!(cursor_id(&app), "!a:example.org");
    keys(&mut app, "gg");
    assert_eq!(cursor_id(&app), "!c:example.org");
    keys(&mut app, "G");
    assert_eq!(cursor_id(&app), "!a:example.org");
}

#[test]
fn half_page_uses_the_last_viewport() {
    let mut app = app();
    for i in 0..20 {
        app.update(Msg::RoomUpserted(room(&format!("!r{i:02}:example.org"), 100 + i, 0)));
    }
    keys(&mut app, "gg");
    app.update(Msg::Viewport { pane: Pane::RoomList, first: 0, last: 9 });
    keys(&mut app, "<C-d>");
    assert_eq!(app.room_cursor(), Some(5));
    keys(&mut app, "2<C-d>");
    assert_eq!(app.room_cursor(), Some(15));
}

#[test]
fn cursor_follows_its_room_when_the_order_changes() {
    let mut app = app();
    keys(&mut app, "j"); // on !b
    app.update(Msg::RoomUpserted(room("!b:example.org", 99, 1)));
    assert_eq!(cursor_id(&app), "!b:example.org");
    assert_eq!(app.room_cursor(), Some(0));
}

#[test]
fn opening_a_room_subscribes_and_focuses_the_timeline() {
    let mut app = app();
    let fx = keys(&mut app, "j<CR>");
    assert_eq!(fx, vec![Effect::Matrix(MatrixEffect::SubscribeTimeline(id("!b:example.org")))]);
    assert_eq!(app.open_room(), Some(&id("!b:example.org")));
    assert_eq!(app.focus(), Focus::Timeline);
    let fx = keys(&mut app, "<C-w>hgg<CR>");
    assert_eq!(no_timers(fx), vec![
        Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id("!b:example.org"))),
        Effect::Matrix(MatrixEffect::SubscribeTimeline(id("!c:example.org"))),
    ]);
}

#[test]
fn reopening_the_open_room_only_moves_focus() {
    let mut app = app();
    keys(&mut app, "<CR>");
    let fx = keys(&mut app, "<C-w>h<CR>");
    assert_eq!(no_timers(fx), vec![]);
    assert_eq!(app.focus(), Focus::Timeline);
}

#[test]
fn open_next_unread_wraps_and_skips_read_rooms() {
    let next = Binding::single(Action::from_spec("room.open", &[
        ("filter".into(), crate::action::ArgValue::String("unread".into())),
        ("pick".into(), crate::action::ArgValue::String("next".into())),
    ]).unwrap());
    let mut app = app_with(vec![(TableId::Normal, seq("]u"), next)]);
    keys(&mut app, "]u");
    assert_eq!(app.open_room(), Some(&id("!c:example.org")));
    keys(&mut app, "]u");
    assert_eq!(app.open_room(), Some(&id("!a:example.org")));
    keys(&mut app, "]u");
    assert_eq!(app.open_room(), Some(&id("!c:example.org")));
}

#[test]
fn mark_read_then_next_unread_is_deterministic() {
    let next = Action::from_spec("room.open", &[
        ("filter".into(), crate::action::ArgValue::String("unread".into())),
        ("pick".into(), crate::action::ArgValue::String("next".into())),
    ]).unwrap();
    let both = Binding::new(vec![Action::RoomMarkRead, next]).unwrap();
    let mut app = app_with(vec![(TableId::Normal, seq("<Space>x"), both)]);
    keys(&mut app, "<CR>"); // open !c (unread 2)
    let fx = no_timers(keys(&mut app, "<Space>x"));
    assert_eq!(fx, vec![
        Effect::Matrix(MatrixEffect::MarkRoomRead(id("!c:example.org"))),
        Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id("!c:example.org"))),
        Effect::Matrix(MatrixEffect::SubscribeTimeline(id("!a:example.org"))),
    ]);
    assert_eq!(app.rooms().get(&id("!c:example.org")).unwrap().unread, UnreadCounts::default());
}

#[test]
fn mark_read_targets_the_cursor_room_in_the_room_list() {
    let mut app = app_with(vec![(TableId::Normal, seq("<Space>r"), Binding::single(Action::RoomMarkRead))]);
    keys(&mut app, "<CR><C-w>hG"); // open !c, back to the room list, cursor on !a
    let fx = no_timers(keys(&mut app, "<Space>r"));
    assert_eq!(fx, vec![Effect::Matrix(MatrixEffect::MarkRoomRead(id("!a:example.org")))]);
    keys(&mut app, "<C-w>l");
    let fx = no_timers(keys(&mut app, "<Space>r"));
    assert_eq!(fx, vec![Effect::Matrix(MatrixEffect::MarkRoomRead(id("!c:example.org")))]);
}

#[test]
fn sidebar_toggle_moves_focus_off_a_hidden_room_list() {
    let mut app = app();
    keys(&mut app, "<Space>e");
    assert!(!app.sidebar_visible());
    assert_eq!(app.focus(), Focus::Timeline);
    keys(&mut app, "<C-w>h");
    assert!(app.sidebar_visible());
    assert_eq!(app.focus(), Focus::RoomList);
}

#[test]
fn removing_the_open_room_unsubscribes_and_keeps_a_cursor() {
    let mut app = app();
    keys(&mut app, "<CR>"); // open !c
    keys(&mut app, "<C-w>h");
    let fx = app.update(Msg::RoomRemoved(id("!c:example.org")));
    assert_eq!(fx, vec![Effect::Matrix(MatrixEffect::UnsubscribeTimeline(id("!c:example.org")))]);
    assert_eq!(app.open_room(), None);
    assert_eq!(cursor_id(&app), "!b:example.org");
}

#[test]
fn quit_and_unavailable_actions() {
    let mut app = app();
    assert_eq!(no_timers(keys(&mut app, "<Space><Space>")).len(), 0);
    assert_eq!(app.status().unwrap().text, "picker.rooms is not available yet");
    let quit = Binding::single(Action::AppQuit);
    let mut app = app_with(vec![(TableId::Normal, seq("Q"), quit)]);
    assert_eq!(keys(&mut app, "Q"), vec![Effect::Ui(UiEffect::Quit)]);
}

#[test]
fn status_clears_on_the_next_key() {
    let mut app = app();
    app.update(Msg::EffectFailed { error: "boom".into() });
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    keys(&mut app, "j");
    assert_eq!(app.status(), None);
}

#[test]
fn timeout_releases_through_the_original_context() {
    let mut app = app();
    let fx = keys(&mut app, "g");
    let Some(Effect::Ui(UiEffect::StartTimer { id, .. })) = fx.last().cloned() else { panic!("{fx:?}") };
    assert_eq!(app.pending_keys(), seq("g").keys());
    assert_eq!(app.update(Msg::Timeout { id }), vec![]);
    assert!(app.pending_keys().is_empty());
}

#[test]
fn editor_contexts_release_keys_to_the_editor() {
    let mut app = app();
    keys(&mut app, "<CR>");
    app.focus_composer_for_test();
    // Insert mode: unbound keys go to edtui; releases from one key are merged.
    let fx = no_timers(keys_in(&mut app, "<C-x>a", Some(EditorMode::Insert)));
    assert_eq!(fx, vec![Effect::Ui(UiEffect::ReplayKeys(seq("<C-x>a").keys().to_vec()))]);
    // Normal mode: `x` is unbound in `composer.normal` and `normal`.
    assert_eq!(keys_in(&mut app, "x", Some(EditorMode::Normal)), vec![
        Effect::Ui(UiEffect::ReplayKeys(seq("x").keys().to_vec())),
    ]);
    // Search mode: every key goes to edtui, even `<Esc>`.
    assert_eq!(keys_in(&mut app, "<Esc>", Some(EditorMode::Search)), vec![
        Effect::Ui(UiEffect::ReplayKeys(seq("<Esc>").keys().to_vec())),
    ]);
    assert_eq!(app.mode_label(Some(EditorMode::Insert)), "INSERT");
}
```

`focus_composer_for_test` is a `#[cfg(test)] pub(crate)` method on `App` that
sets `Focus::Composer` while a room is open; `composer.focus` (phase 3)
replaces it. All `Release` outputs of one `feed` or `timeout` call are merged
into one `ReplayKeys` effect.

- [ ] **Step 2: Run them to see them fail:** `nix develop -c cargo test -p mxt-core app`
- [ ] **Step 3: Implement.** `app.rs` holds the state, accessors, the key
dispatch and the action `match`; `app/rooms.rs` holds the room list actions
as `impl App` methods. Keep `update` a thin `match` over `Msg`.
- [ ] **Step 4: Run all core tests**, then fmt and clippy.
- [ ] **Step 5: Commit**

```bash
git add crates/mxt-core/src
git commit -m "Add the App state and update for room list browsing, so keys resolve to actions and effects without I/O"
```

---

### Task 7: Command line and search

**Files:**
- Create: `crates/mxt-core/src/app/command_line.rs`, `crates/mxt-core/src/app/search.rs`
- Modify: `crates/mxt-core/src/app.rs`, `crates/mxt-core/src/app/tests.rs`

**Interfaces:**
- Consumes: `parse_command`, `COMMANDS`, `takes_argument` (Task 4), `fuzzy::rank` (Task 2), `App` (Task 6).
- Produces:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind { Command, Search(Pane) }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLine { pub kind: LineKind, pub text: String }
impl App {
    /// The open command line, rendered as `:` or `/` followed by `text`.
    pub fn command_line(&self) -> Option<&CommandLine>;
}
```

Both types live in `app/command_line.rs` and are re-exported from `app.rs`
(`pub use command_line::{CommandLine, LineKind};`).

**Behavior:**
- `command_line.open`: open an empty `Command` line. `search.start`: open an
  empty `Search(<focused pane>)` line (room list or timeline; from the
  composer it does nothing).
- Keys released to the `Text` sink while a line is open: a key with
  `Key::text()` appends the character; `<BS>` deletes the last character, or
  closes the line when it is empty; `<C-u>` clears the text; `<C-w>` deletes
  the last word (trailing whitespace, then non-whitespace). Other keys are
  ignored. Every edit resets completion.
- `command_line.cancel`: close the line.
- `command_line.execute`: close the line, then for `Command`:
  `parse_command(text)`: `Ok(Some(action))` runs it (count 1), `Ok(None)` does
  nothing, `Err(e)` sets an `Error` status with `e`'s message. For
  `Search(pane)`: search with the text (an empty text reuses the last
  pattern).
- `command_line.complete` (`Command` lines only): the first press computes
  the candidates and remembers the text as typed:
  - no whitespace in the text: the `COMMANDS` that start with it, each
    completed as the name plus a space when `takes_argument`;
  - text `open <query>`: the displayed names of all rooms ranked with
    `fuzzy::rank(query, names)`, each completed as `open <name>`.
  Each press replaces the text with the next candidate; after the last one the
  typed text comes back, then the cycle repeats. A single candidate is
  inserted and ends the completion, so the next press computes fresh
  candidates (`:o<Tab><Tab>` gives `open `, then room names). No candidates:
  nothing happens.
- Search (room list): case-insensitive substring match (`to_lowercase` on
  both sides) on the displayed name, starting after the cursor and wrapping
  around (the cursor's own room is checked last). Found: move the cursor.
  Not found: `Error` status `Pattern not found: <pattern>`. The pattern is
  remembered as the last search.
- `search.next` / `search.prev`: repeat the last pattern forward / backward in
  the focused pane. Without one: `Error` status `No previous search`.
- Timeline search behaves the same once the timeline exists (phase 2); until
  then it reports `Pattern not found`.
- `mode_label` is `COMMAND` while a line is open.

- [ ] **Step 1: Write the failing tests** (append to `app/tests.rs`)

```rust
fn line(app: &App) -> Option<(LineKind, &str)> {
    app.command_line().map(|l| (l.kind, l.text.as_str()))
}

#[test]
fn command_line_runs_a_command() {
    let mut app = app();
    keys(&mut app, ":open b");
    assert_eq!(line(&app), Some((LineKind::Command, "open b")));
    assert_eq!(app.mode_label(None), "COMMAND");
    let fx = keys(&mut app, "<CR>");
    assert_eq!(fx, vec![Effect::Matrix(MatrixEffect::SubscribeTimeline(id("!b:example.org")))]);
    assert_eq!(line(&app), None);
}

#[test]
fn command_line_keys_do_not_trigger_bindings() {
    let mut app = app();
    keys(&mut app, ":jjgg");
    assert_eq!(line(&app), Some((LineKind::Command, "jjgg")));
    assert_eq!(cursor_id(&app), "!c:example.org");
}

#[test]
fn command_line_editing() {
    let mut app = app();
    keys(&mut app, ":open foo bar<C-w>");
    assert_eq!(line(&app), Some((LineKind::Command, "open foo ")));
    keys(&mut app, "<C-u>");
    assert_eq!(line(&app), Some((LineKind::Command, "")));
    keys(&mut app, "a<BS>");
    assert_eq!(line(&app), Some((LineKind::Command, "")));
    keys(&mut app, "<BS>");
    assert_eq!(line(&app), None);
}

#[test]
fn command_errors_go_to_the_status_line() {
    let mut app = app();
    keys(&mut app, ":quit<CR>");
    assert_eq!(app.status().unwrap().kind, StatusKind::Error);
    // `:q` quits.
    assert_eq!(keys(&mut app, ":q<CR>"), vec![Effect::Ui(UiEffect::Quit)]);
}

#[test]
fn escape_cancels_the_command_line() {
    let mut app = app();
    keys(&mut app, ":open b<Esc>");
    assert_eq!(line(&app), None);
    assert_eq!(app.open_room(), None);
}

#[test]
fn completes_command_names_and_cycles_back() {
    let mut app = app();
    keys(&mut app, ":u");
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "unmute")));
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "upload ")));
    keys(&mut app, "<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "u")));
}

#[test]
fn completes_room_names_for_open() {
    let mut app = app();
    keys(&mut app, ":open c<Tab>");
    assert_eq!(line(&app), Some((LineKind::Command, "open c:example.org")));
}

#[test]
fn search_moves_the_cursor_and_repeats() {
    let mut app = app();
    keys(&mut app, "/A:<CR>");
    assert_eq!(cursor_id(&app), "!a:example.org");
    keys(&mut app, "n");
    assert_eq!(cursor_id(&app), "!a:example.org"); // only match: wraps onto itself
    keys(&mut app, "/example<CR>");
    assert_eq!(cursor_id(&app), "!c:example.org"); // after !a, wrapping
    keys(&mut app, "n");
    assert_eq!(cursor_id(&app), "!b:example.org");
    keys(&mut app, "N");
    assert_eq!(cursor_id(&app), "!c:example.org");
}

#[test]
fn search_failures() {
    let mut app = app();
    keys(&mut app, "n");
    assert_eq!(app.status().unwrap().text, "No previous search");
    keys(&mut app, "/zzz<CR>");
    assert_eq!(app.status().unwrap().text, "Pattern not found: zzz");
}
```

- [ ] **Step 2: Run them to see them fail:** `nix develop -c cargo test -p mxt-core app`
- [ ] **Step 3: Implement** `app/command_line.rs` (line editing, execute,
completion state) and `app/search.rs`; route the `Text` sink and the new
actions from `app.rs`.
- [ ] **Step 4: Run all core tests**, then fmt and clippy.
- [ ] **Step 5: Commit**

```bash
git add crates/mxt-core/src
git commit -m "Add the command line with completion and pane search, so commands and / searches resolve through the same actions as keys"
```

---

### Task 8: Configuration loading

**Files:**
- Modify: root `Cargo.toml` (`serde = { version = "1", features = ["derive"] }`,
  `toml = "1"` in `[workspace.dependencies]`),
  `crates/mxt/Cargo.toml`, `crates/mxt/src/main.rs`
- Create: `crates/mxt/src/config.rs` (paths, loading, conversion, errors),
  `crates/mxt/src/config/dto.rs` (serde DTOs), `crates/mxt/src/config/tests.rs`

**Interfaces:**
- Consumes: `KeySeq::parse`, `Action::from_spec`, `ArgValue`, `Binding`,
  `Keymap::new`, `TableId`, `Settings`, `RoomSort`, `ImagePreview`,
  `ReceiptKind`, `UserId`.
- Produces:

```rust
pub struct Config {
    pub user_id: UserId,
    /// `None`: discovered from the user ID's server name via .well-known.
    pub homeserver: Option<String>,
    pub keymap: Keymap,
    pub settings: Settings,
    /// strftime format, applied in the local time zone by `mxt-tui`.
    pub time_format: String,
    pub download_dir: PathBuf,
    pub opener: Vec<String>,
}

/// `$XDG_CONFIG_HOME/mxt/config.toml`, or `$HOME/.config/mxt/config.toml` when
/// `XDG_CONFIG_HOME` is unset or empty (the same on macOS).
pub fn default_path(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf>;
/// Reads and converts the file. `home` expands a leading `~` in paths.
pub fn load(path: &Path, home: &Path) -> Result<Config, ConfigError>;
/// The same for a string, for tests; `path` is only used in messages.
pub fn parse(text: &str, path: &Path, home: &Path) -> Result<Config, ConfigError>;

pub enum ConfigError {
    NotFound { path: PathBuf },
    Read { path: PathBuf, error: std::io::Error },
    /// `line` and `column` are 1-based.
    Invalid { path: PathBuf, line: usize, column: usize, message: String },
}
impl std::fmt::Display for ConfigError;   // Invalid: "<path>:<line>:<column>: <message>"

/// Printed with the path when the file does not exist.
pub const MINIMAL_EXAMPLE: &str = "[account]\nuser_id = \"@alice:example.org\"\n";
```

**The file** (design, "Configuration"; every table uses
`deny_unknown_fields`):

| Key | Type | Default |
|---|---|---|
| `account.user_id` | string `@local:server` | required |
| `account.homeserver` | URL string | none |
| `ui.leader` | key notation | `"<Space>"` |
| `ui.timeoutlen` | integer, milliseconds | `1000` |
| `ui.room_list.visible` | boolean | `true` |
| `ui.room_list.sort` | `"recent"` or `"name"` | `"recent"` |
| `ui.time_format` | string | `"%H:%M"` |
| `ui.image_preview` | `"auto"` or `"off"` | `"auto"` |
| `ui.read_receipts` | `"public"` or `"private"` | `"public"` |
| `files.download_dir` | path, leading `~` expanded | `"~/Downloads"` |
| `files.opener` | array of strings, non-empty | `["open"]` on macOS, `["xdg-open"]` elsewhere |
| `keymap.<table>` | tables `normal`, `room_list`, `timeline`, `message_view`, `picker`, `command_line`, and `keymap.composer.{normal,visual,insert,completion}` | none |

Binding values: a string (an action without arguments, or `"nop"`), a table
with `action` and the action's arguments, or a non-empty array of strings and
tables. Argument values convert to `ArgValue` (string, integer, boolean);
other TOML types are errors. Keys are parsed with `KeySeq::parse(key,
&leader)` after the leader itself is parsed.

**Errors** are `Invalid` with the line and column of the offending value or
key, and a message that names the setting: unknown fields (from
`deny_unknown_fields`), wrong types, an invalid `user_id`, invalid key
notation (`keymap.timeline: invalid key "<C-x": ...`), unknown actions, wrong
or conflicting arguments (the `ActionError` message), empty arrays. Use
`toml::Spanned` (or `toml`'s span-preserving document API) to keep spans for
keymap keys and values, and convert byte offsets to line and column.

**`main`:** `mxt [--config PATH]` and `mxt --version` (prints
`mxt <version>`); anything else prints a usage line and exits with status 2.
Without `--config`, `default_path`. On `NotFound`, print the path and
`MINIMAL_EXAMPLE` to stderr and exit with status 1; on other errors print the
error and exit with status 1. With a valid configuration, exit with status 0
for now (the startup tasks continue from here).

- [ ] **Step 1: Write the failing tests** (`config/tests.rs`)

```rust
use super::*;
use mxt_core::action::Action;
use mxt_core::keymap::InputContext;
use mxt_core::keys::KeySeq;
use mxt_core::rooms::RoomSort;
use mxt_core::settings::{ImagePreview, ReceiptKind};
use std::path::{Path, PathBuf};

fn ok(text: &str) -> Config {
    parse(text, Path::new("config.toml"), Path::new("/home/alice")).unwrap_or_else(|e| panic!("{e}"))
}
fn err(text: &str) -> (usize, String) {
    match parse(text, Path::new("config.toml"), Path::new("/home/alice")) {
        Err(ConfigError::Invalid { line, message, .. }) => (line, message),
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("accepted: {text}"),
    }
}
fn lookup<'a>(c: &'a Config, ctx: InputContext, keys: &str) -> Option<Vec<&'static str>> {
    let leader = KeySeq::parse("<Space>", &KeySeq::default()).unwrap();
    let seq = KeySeq::parse(keys, &leader).unwrap();
    c.keymap.effective(ctx).lookup(seq.keys()).binding.map(|b| b.actions().iter().map(Action::name).collect())
}

const MINIMAL: &str = "[account]\nuser_id = \"@alice:example.org\"\n";

#[test]
fn minimal_file_uses_defaults() {
    let c = ok(MINIMAL);
    assert_eq!(c.user_id.as_str(), "@alice:example.org");
    assert_eq!(c.homeserver, None);
    assert_eq!(c.settings.timeoutlen_ms, 1000);
    assert_eq!(c.settings.room_sort, RoomSort::Recent);
    assert!(c.settings.room_list_visible);
    assert_eq!(c.settings.image_preview, ImagePreview::Auto);
    assert_eq!(c.settings.read_receipts, ReceiptKind::Public);
    assert_eq!(c.time_format, "%H:%M");
    assert_eq!(c.download_dir, PathBuf::from("/home/alice/Downloads"));
    assert!(!c.opener.is_empty());
    assert_eq!(lookup(&c, InputContext::Timeline, "j"), Some(vec!["cursor.down"]));
}

#[test]
fn the_design_example_loads() {
    let c = ok(r#"
[account]
user_id = "@alice:example.org"
# homeserver = "https://matrix.example.org"

[ui]
leader = "<Space>"
timeoutlen = 1000
room_list = { visible = true, sort = "recent" }
time_format = "%H:%M"
image_preview = "auto"
read_receipts = "public"

[files]
download_dir = "~/Downloads"
opener = ["xdg-open"]

[keymap.normal]
"]u" = { action = "room.open", filter = "unread", pick = "next" }
"<leader>x" = ["room.mark_read", { action = "room.open", filter = "unread", pick = "next" }]

[keymap.timeline]
"dd" = "nop"
"D" = "message.delete"

[keymap.composer.insert]
"<C-s>" = "composer.send"
"#);
    assert_eq!(lookup(&c, InputContext::Timeline, "]u"), Some(vec!["room.open"]));
    assert_eq!(lookup(&c, InputContext::RoomList, "<Space>x"), Some(vec!["room.mark_read", "room.open"]));
    assert_eq!(lookup(&c, InputContext::Timeline, "dd"), Some(vec!["nop"]));
    assert_eq!(lookup(&c, InputContext::Timeline, "D"), Some(vec!["message.delete"]));
    assert_eq!(lookup(&c, InputContext::ComposerInsert, "<C-s>"), Some(vec!["composer.send"]));
    assert_eq!(lookup(&c, InputContext::ComposerInsert, "<CR>"), Some(vec!["composer.send"]));
}

#[test]
fn settings_are_read() {
    let c = ok(&format!("{MINIMAL}[ui]\ntimeoutlen = 300\nread_receipts = \"private\"\nroom_list = {{ visible = false, sort = \"name\" }}\nimage_preview = \"off\"\n[files]\ndownload_dir = \"/tmp/dl\"\nopener = [\"wslview\"]\n"));
    assert_eq!(c.settings.timeoutlen_ms, 300);
    assert_eq!(c.settings.read_receipts, ReceiptKind::Private);
    assert_eq!(c.settings.room_sort, RoomSort::Name);
    assert!(!c.settings.room_list_visible);
    assert_eq!(c.settings.image_preview, ImagePreview::Off);
    assert_eq!(c.download_dir, PathBuf::from("/tmp/dl"));
    assert_eq!(c.opener, vec!["wslview".to_string()]);
}

#[test]
fn leader_applies_to_defaults_and_user_bindings() {
    let c = ok(&format!("{MINIMAL}[ui]\nleader = \",\"\n[keymap.normal]\n\"<leader>q\" = \"app.quit\"\n"));
    let comma = KeySeq::parse(",", &KeySeq::default()).unwrap();
    let seq = KeySeq::parse("<leader>q", &comma).unwrap();
    let names: Vec<_> = c.keymap.effective(InputContext::Timeline).lookup(seq.keys()).binding.unwrap().actions().iter().map(Action::name).collect();
    assert_eq!(names, vec!["app.quit"]);
    let e = KeySeq::parse("<leader>e", &comma).unwrap();
    assert!(c.keymap.effective(InputContext::Timeline).lookup(e.keys()).binding.is_some());
}

#[test]
fn errors_name_the_line() {
    let (line, msg) = err(&format!("{MINIMAL}[ui]\ntimeoutln = 5\n"));
    assert_eq!(line, 4);
    assert!(msg.contains("timeoutln"), "{msg}");

    let (line, msg) = err(&format!("{MINIMAL}[keymap.timeline]\n\"x\" = \"message.dlete\"\n"));
    assert_eq!(line, 4);
    assert!(msg.contains("message.dlete"), "{msg}");

    let (line, msg) = err(&format!("{MINIMAL}[keymap.normal]\n\"]u\" = {{ action = \"room.open\", pick = \"next\", query = \"x\" }}\n"));
    assert_eq!(line, 4);
    assert!(msg.contains("pick") && msg.contains("query"), "{msg}");

    let (line, msg) = err(&format!("{MINIMAL}[keymap.normal]\n\"<C-x\" = \"nop\"\n"));
    assert_eq!(line, 4);
    assert!(msg.contains("<C-x"), "{msg}");

    let (line, _) = err(&format!("{MINIMAL}[keymap.normal]\n\"x\" = []\n"));
    assert_eq!(line, 4);

    let (line, _) = err(&format!("{MINIMAL}[keymap.composer.foo]\n\"x\" = \"nop\"\n"));
    assert_eq!(line, 3);

    let (line, _) = err(&format!("{MINIMAL}[keymap.normal]\n\"x\" = {{ action = \"room.open\", filter = 3.5 }}\n"));
    assert_eq!(line, 4);

    let (line, msg) = err("[account]\nuser_id = \"alice\"\n");
    assert_eq!(line, 2);
    assert!(msg.contains("user_id"), "{msg}");

    let (line, _) = err("[account]\n");
    assert_eq!(line, 1);
}

#[test]
fn default_path_follows_xdg() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    };
    assert_eq!(
        default_path(&env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/home/alice")])),
        Some(PathBuf::from("/x/mxt/config.toml"))
    );
    assert_eq!(
        default_path(&env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/alice")])),
        Some(PathBuf::from("/home/alice/.config/mxt/config.toml"))
    );
    assert_eq!(default_path(&env(&[])), None);
}
```

- [ ] **Step 2: Run them to see them fail:** `nix develop -c cargo test -p mxt`
- [ ] **Step 3: Implement** `config.rs`, `config/dto.rs` and the `main`
command line. Keep the DTOs private to the module; only `Config`, the
functions and `ConfigError` are public.
- [ ] **Step 4: Run the tests**, then fmt, clippy, and by hand:
`nix develop -c cargo run -p mxt -- --config /nonexistent` (prints the path
and the example, status 1), `... -- --version`, and a file with a typo
(prints `path:line:column: message`, status 1).
- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/mxt
git commit -m "Load the configuration into checked core types, so typos and invalid bindings are reported with their line before the UI starts"
```

---

### Task 9: Terminal adapter and the event loop

The room list pane, the status line and the command line, drawn from the
core state, and the event loop in `mxt` that connects terminal input, timers
and (later) the matrix adapter to `App::update`. The matrix adapter is not
part of this task: the loop takes a channel of `Msg` from it and a channel
for `MatrixEffect`s, which stay unused until the sync task.

**Files:**
- Modify: root `Cargo.toml` (`ratatui = "0.30"`, `crossterm = { version =
  "0.29", features = ["event-stream"] }`, `tokio = { version = "1", features =
  ["rt-multi-thread", "macros", "sync", "time"] }`, `futures-util = "0.3"`,
  `unicode-width = "0.2"` in `[workspace.dependencies]`; use the crossterm
  version that ratatui 0.30 uses, so there is one crossterm in the tree),
  `crates/mxt-tui/Cargo.toml`, `crates/mxt/Cargo.toml`, `crates/mxt/src/main.rs`
- Create: `crates/mxt-tui/src/terminal.rs`, `crates/mxt-tui/src/input.rs`,
  `crates/mxt-tui/src/view.rs` (layout and drawing), `crates/mxt-tui/src/timers.rs`,
  `crates/mxt/src/event_loop.rs`

**Interfaces:**
- Consumes: `App` and its accessors, `Msg`, `Effect`, `UiEffect`,
  `MatrixEffect`, `Key::new`, `KeyCode`, `Modifiers`, `Badge`,
  `NotificationMode::label`, `TimerId` (Tasks 2–7).
- Produces:

```rust
// mxt-tui
pub struct Tui { /* private: ratatui terminal, list scroll state, last viewports, timers */ }
impl Tui {
    /// Raw mode, alternate screen, focus change reporting, hidden cursor.
    pub fn new() -> std::io::Result<Tui>;
    /// Draws the frame and returns `Msg::Viewport` for panes whose visible
    /// range changed since the last draw.
    pub fn draw(&mut self, app: &App) -> std::io::Result<Vec<Msg>>;
    /// Converts one terminal event. Key presses and repeats become `Msg::Key`
    /// (`editor: None` until the composer exists), focus events become
    /// `Msg::TerminalFocus`; resizes and releases produce nothing (the loop
    /// redraws after every event anyway).
    pub fn handle_event(&mut self, app: &App, event: crossterm::event::Event) -> Vec<Msg>;
    /// Executes a UI effect. `StartTimer` schedules a deadline; `ReplayKeys`
    /// goes to the focused editor (no editors exist before phase 3, so it is
    /// dropped); `Quit` sets `should_quit`.
    pub fn execute(&mut self, effect: UiEffect, now: std::time::Instant);
    pub fn should_quit(&self) -> bool;
    /// The earliest pending timer deadline.
    pub fn next_deadline(&self) -> Option<std::time::Instant>;
    /// Removes and returns `Msg::Timeout` for every timer due at `now`.
    pub fn expired(&mut self, now: std::time::Instant) -> Vec<Msg>;
    /// Leaves raw mode and the alternate screen. Also run by `Drop`.
    pub fn restore(&mut self) -> std::io::Result<()>;
}
/// Restores the terminal before the default hook prints the panic.
pub fn install_panic_hook();
/// crossterm key event to a core key; `None` for releases and keys the core
/// has no notation for.
pub fn key_from_crossterm(event: crossterm::event::KeyEvent) -> Option<Key>;

// mxt
/// Runs until `UiEffect::Quit`. `from_matrix` delivers adapter messages;
/// matrix effects are sent to `to_matrix`.
pub async fn run(
    app: &mut App,
    tui: &mut Tui,
    from_matrix: tokio::sync::mpsc::UnboundedReceiver<Msg>,
    to_matrix: tokio::sync::mpsc::UnboundedSender<MatrixEffect>,
) -> std::io::Result<()>;
```

**Layout and drawing** (design, "Layout", "Room names", "Unread badges";
colors only from the 16-color palette):
- Bottom row: the status line, or the command line while one is open
  (`:` or `/` followed by the text; the terminal cursor is shown at the end of
  the text, placed by display width).
- Above it, left: the room list when `sidebar_visible` (30 columns, or a
  third of the width on terminals narrower than 90 columns), with a border on
  its right. Right: the timeline pane. Until phase 2 the timeline pane shows
  the open room's displayed name as a header and `No room open` when none is.
- Room list rows: the displayed name (`network / room`), truncated by display
  width with `…`; right-aligned the badge (`Badge::from_counts`, `text`) and
  the mode label. Styles: mentions badge bold red; notification badge bold;
  message badge dim; marked badge yellow; labels dim. The cursor row is
  reversed while the room list has focus and underlined otherwise; the open
  room's name is bold.
- The room list scrolls so the cursor stays visible (a ratatui `ListState`).
  After drawing, its first and last visible indices are compared with the
  last reported ones; a change yields `Msg::Viewport { pane: RoomList, .. }`.
- Status line: the mode label (reversed), the pending count and keys (like
  nvim's showcmd), the open room's displayed name, the status message (red
  for `Error`), and right-aligned the sync state (`starting`, `synced`,
  `offline`, `retrying`).

**Key conversion:** `KeyEventKind::Press` and `Repeat` only. crossterm
`Char(c)`, `Enter`, `Esc`, `Tab`, `BackTab` (Tab with shift), `Backspace`,
`Delete`, `Insert`, arrows, `Home`, `End`, `PageUp`, `PageDown`, `F(n)`, with
ctrl, alt and shift from the modifiers, through `Key::new` so normalization
happens in one place. Other codes return `None`.

**Event loop** (`event_loop::run`): draw once, then repeat until
`tui.should_quit()`: wait with `tokio::select!` for a crossterm
`EventStream` event, a message from `from_matrix`, or the next timer deadline
(`tokio::time::sleep_until`, skipped when there is none); turn it into `Msg`s
(`handle_event`, the received message, or `expired`); feed each to
`app.update`, and route the effects (`Effect::Ui` to `tui.execute`,
`Effect::Matrix` to `to_matrix`); then draw and feed the returned viewport
messages through `update` as well (they produce no effects in this phase, but
route them the same way). Drain every message already queued on
`from_matrix` before drawing, so a burst of room updates draws once.

**`main`:** after the configuration loads, build the `App`
(`App::new(config.keymap, config.settings)`), install the panic hook, create
the `Tui`, create the two channels (keep the sender of `from_matrix` alive
in `main` until the sync task uses it), run the loop on a multi-threaded tokio
runtime, and restore the terminal on exit.

- [ ] **Step 1: Write the failing tests** (`input.rs`; rendering is verified
by running, per the design)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode as C, KeyEvent, KeyEventKind, KeyModifiers as M};
    use mxt_core::keys::{Key, KeySeq};

    fn key(text: &str) -> Key {
        KeySeq::parse(text, &KeySeq::default()).unwrap().keys()[0]
    }
    fn press(code: C, mods: M) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn characters_and_shift() {
        assert_eq!(key_from_crossterm(press(C::Char('j'), M::NONE)), Some(key("j")));
        assert_eq!(key_from_crossterm(press(C::Char('G'), M::SHIFT)), Some(key("G")));
        assert_eq!(key_from_crossterm(press(C::Char('あ'), M::NONE)), Some(key("あ")));
        assert_eq!(key_from_crossterm(press(C::Char(' '), M::NONE)), Some(key("<Space>")));
    }

    #[test]
    fn control_and_special_keys() {
        assert_eq!(key_from_crossterm(press(C::Char('w'), M::CONTROL)), Some(key("<C-w>")));
        assert_eq!(key_from_crossterm(press(C::Enter, M::NONE)), Some(key("<CR>")));
        assert_eq!(key_from_crossterm(press(C::BackTab, M::SHIFT)), Some(key("<S-Tab>")));
        assert_eq!(key_from_crossterm(press(C::Esc, M::NONE)), Some(key("<Esc>")));
        assert_eq!(key_from_crossterm(press(C::F(5), M::NONE)), Some(key("<F5>")));
        assert_eq!(key_from_crossterm(press(C::Char('x'), M::ALT | M::CONTROL)), Some(key("<C-A-x>")));
    }

    #[test]
    fn releases_are_ignored() {
        let mut e = press(C::Char('j'), M::NONE);
        e.kind = KeyEventKind::Release;
        assert_eq!(key_from_crossterm(e), None);
    }
}
```

Also unit-test `timers.rs` (deadlines ordered, `expired` returns only due
timers, in deadline order).

- [ ] **Step 2: Run them to see them fail:** `nix develop -c cargo test -p mxt-tui`
- [ ] **Step 3: Implement** `mxt-tui` and the loop in `mxt`.
- [ ] **Step 4: Verify by running.** Write a throwaway example (not
committed) that feeds a few `Msg::RoomUpserted` with made-up rooms (a muted
room with unread messages, a room with mentions, a marked room, a
`network / room` name, a long Japanese name) into the channel and runs the
loop. Run it in a tmux window of session `mxt` (never session `0`), drive it
with `tmux send-keys` (`j`, `k`, `G`, `gg`, `<Space>e`, `:open`, `<Tab>`,
`<Esc>`, `/`, `n`, `<CR>`, `:q<CR>`), and check `tmux capture-pane -p` after
each step: badges and labels, cursor movement and scrolling with more rooms
than rows, the hidden sidebar, the command line and its cursor, search, and a
clean exit that restores the terminal. Put the captured panes in the pull
request description. Delete the example before committing.
- [ ] **Step 5: Run all tests**, then fmt and clippy.
- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/mxt-tui crates/mxt
git commit -m "Add the terminal adapter and the event loop, so the room list, status line and command line are drawn from the core state"
```

---

### Task 10: Startup building blocks (client, session, recovery, paths, lock, logging)

The pieces of the design's "Startup", "Storage" and "Logging" sections, as
library code with unit tests. Wiring them into `main` is Task 12, after the
configuration (Task 8) and the event loop (Task 9) have landed, so this task
does not touch `crates/mxt/src/main.rs`.

The SDK API details below come from the phase 0 spike (matrix-sdk 0.19.1,
checked in the crate sources).

**Files:**
- Modify: root `Cargo.toml` (`[workspace.dependencies]`: `matrix-sdk = {
  version = "0.19", default-features = false, features = ["e2e-encryption",
  "sqlite", "rustls-aws-lc-rs"] }`, `matrix-sdk-ui = { version = "0.19",
  default-features = false }`, `serde_json = "1"`, `rpassword = "7"`,
  `tracing = "0.1"`, `tracing-subscriber = { version = "0.3", features =
  ["env-filter", "fmt"] }`, `tracing-appender = "0.2"`, and `tokio` if Task 9
  has not added it yet), `deny.toml`, `crates/mxt-matrix/Cargo.toml`,
  `crates/mxt/Cargo.toml`
- Create: `crates/mxt-matrix/src/client.rs`, `crates/mxt-matrix/src/session.rs`,
  `crates/mxt-matrix/src/recovery.rs`, `crates/mxt-matrix/src/error.rs`,
  `crates/mxt/src/paths.rs`, `crates/mxt/src/lock.rs`,
  `crates/mxt/src/logging.rs`, `crates/mxt/src/invalidate.rs`,
  `crates/mxt/src/prompt.rs`
- Modify: `crates/mxt-matrix/src/lib.rs`; declare the new `mxt` modules
  where `main.rs` already declares modules, or in a `lib.rs`-free way that
  does not conflict with Task 8 (add `mod paths; mod lock; ...` lines next to
  the existing `mod` lines, with `#[allow(dead_code)]` on items `main` does
  not use yet; Task 12 removes the allows)

**Interfaces:**

```rust
// mxt-matrix
pub use matrix_sdk::Client;

pub enum Homeserver {
    /// Discover from the user ID's server name via .well-known.
    Discover(String),
    Url(String),
}
pub struct StoreDirs { pub data: PathBuf, pub cache: PathBuf }

/// SQLite state and crypto stores in `dirs.data` without a passphrase, the
/// event cache and media cache in `dirs.cache`
/// (`ClientBuilder::sqlite_store_with_cache_path`). Encryption settings from
/// the design: `backup_download_strategy: AfterDecryptionFailure`,
/// `auto_enable_backups: false`, `auto_enable_cross_signing: false`. Sliding
/// sync discovered natively (`VersionBuilder::DiscoverNative`). No refresh
/// tokens.
pub async fn build_client(homeserver: &Homeserver, dirs: &StoreDirs) -> Result<Client, Error>;

/// What `session.json` holds: the homeserver URL after login (the login
/// response's well-known can change it) and the SDK session.
#[derive(Serialize, Deserialize)]
pub struct SavedSession { pub homeserver: String, pub session: matrix_sdk::authentication::matrix::MatrixSession }
impl std::fmt::Debug for SavedSession;   // never prints tokens

/// `Ok(None)` when the file does not exist.
pub fn load_session(path: &Path) -> Result<Option<SavedSession>, Error>;
/// Writes a temporary file with mode 0600 next to `path`, then renames it.
pub fn save_session(path: &Path, session: &SavedSession) -> Result<(), Error>;
/// Password login, device display name "mxt", no refresh token. Fails with
/// `Error::PasswordLoginUnsupported { offered }` when the server does not
/// offer `m.login.password`.
pub async fn login(client: &Client, user_id: &str, password: &str) -> Result<SavedSession, Error>;
pub async fn restore(client: &Client, session: SavedSession) -> Result<(), Error>;

pub enum RecoveryStatus { Enabled, Incomplete, Disabled, Unknown }
/// Waits for the E2EE initialization tasks first (the state is `Unknown` before).
pub async fn recovery_status(client: &Client) -> RecoveryStatus;
pub async fn recover(client: &Client, key: &str) -> Result<(), Error>;
/// Polls a one-off `/keys/query` for the own device until its keys are on
/// the server or `timeout` passes. `Recovery::recover` signs the device, and
/// that upload needs the device keys on the server, so the first start waits
/// for this before asking for the recovery key (found in phase 0).
pub async fn wait_for_device_keys(client: &Client, timeout: Duration) -> Result<bool, Error>;

pub enum Error { /* thiserror-free enum with Display; variants for the SDK error, I/O, JSON, PasswordLoginUnsupported { offered: Vec<String> }, UnknownToken */ }
impl Error { pub fn is_unknown_token(&self) -> bool; }

// mxt
pub struct Paths { pub data: PathBuf, pub cache: PathBuf, pub state: PathBuf }
impl Paths {
    /// `$XDG_DATA_HOME/mxt`, `$XDG_CACHE_HOME/mxt`, `$XDG_STATE_HOME/mxt`, with
    /// `~/.local/share`, `~/.cache`, `~/.local/state` when a variable is unset
    /// or empty; the same on macOS. `None` without `HOME`.
    pub fn from_env(env: &impl Fn(&str) -> Option<String>) -> Option<Paths>;
    pub fn session_file(&self) -> PathBuf;   // data/session.json
    pub fn lock_file(&self) -> PathBuf;      // data/lock
    pub fn store_dir(&self) -> PathBuf;      // data/store
    /// Creates the three directories with mode 0700 (and tightens an existing
    /// data directory to 0700).
    pub fn create_dirs(&self) -> std::io::Result<()>;
}

pub struct DataLock { /* the open lock file */ }
pub enum LockError { Held, Io(std::io::Error) }
impl DataLock {
    /// `File::try_lock` on `path`; `Held` when another process holds it.
    pub fn acquire(path: &Path) -> Result<DataLock, LockError>;
}

/// Daily files `mxt.<date>.log` in `state_dir`, seven kept
/// (`tracing_appender::rolling::Builder` with `max_log_files(7)`). `filter` is
/// `MXT_LOG`; the default is `warn,mxt=info,mxt_core=info,mxt_matrix=info,mxt_tui=info`
/// (info for mxt, warn for matrix-rust-sdk and everything else). Nothing is
/// written to the terminal.
pub fn init_logging(state_dir: &Path, filter: Option<&str>) -> Result<tracing_appender::non_blocking::WorkerGuard, Box<dyn std::error::Error>>;

/// `M_UNKNOWN_TOKEN`: renames the data directory to
/// `<parent>/mxt.invalid-<unix seconds>` and deletes the cache directory.
/// Returns the new path. The caller has dropped the SDK client before.
pub fn invalidate(paths: &Paths, unix_seconds: u64) -> std::io::Result<PathBuf>;

/// Moves a store without a session file (left by a device whose session was
/// lost) to `data/store.invalid-<unix seconds>`, so a new login never reuses
/// another device's crypto store. No-op when there is no store.
pub fn set_aside_orphaned_store(paths: &Paths, unix_seconds: u64) -> std::io::Result<Option<PathBuf>>;

/// Reads a secret from the terminal without echo (`rpassword`).
pub fn prompt_secret(prompt: &str) -> std::io::Result<String>;
```

**`deny.toml`:** this task brings matrix-sdk to `main`, so add:

```toml
[advisories]
ignore = [
  # libsqlite3-sys 0.38.2 is pinned by matrix-sdk-sqlite 0.19 through rusqlite.
  # mxt does not enable the bundled SQLite: Nix builds link the flake's
  # SQLite (3.54.0, which has the fix); other builds link the system SQLite,
  # which must be 3.54.0 or newer.
  { id = "RUSTSEC-2026-0459", reason = "system SQLite is linked; the fix is in SQLite 3.54.0" },
]
```

**Tests** (unit tests; no homeserver):
- `paths.rs`: every variable set; empty values treated as unset; no `HOME`
  gives `None`; the derived file paths.
- `lock.rs`: a second `acquire` on the same file while the first is held
  returns `Held`; after dropping the first, `acquire` succeeds (use a unique
  directory under `std::env::temp_dir()`; clean it up).
- `invalidate.rs`: a data directory with a file is renamed to
  `mxt.invalid-<n>` next to it, and the cache directory is gone;
  `set_aside_orphaned_store` moves `store` only when `session.json` is
  missing.
- `session.rs`: `save_session` then `load_session` round-trips a made-up
  session (`@alice:example.org`, device `ABCDEFGHIJ`, token
  `not-a-real-token`); the file mode is 0600; a missing file loads as `None`;
  `format!("{:?}", session)` does not contain the token.

- [ ] **Step 1: Write the failing tests.**
- [ ] **Step 2: Run them to see them fail.** `mxt` and `mxt-matrix` now
  compile matrix-sdk: run every cargo command under the shared lock.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the tests, fmt and clippy.** Check `build_client` by
  hand with a throwaway test or example (not committed) against a public
  homeserver's discovery, for example `Homeserver::Discover("matrix.org")`
  with temporary directories: it must build without logging in. Do not log in
  anywhere.
- [ ] **Step 5: Commit.** CI's Nix workflow builds the package because
  `Cargo.lock` changed; do not run `nix build` locally.

```bash
git add Cargo.toml Cargo.lock deny.toml crates/mxt-matrix crates/mxt
git commit -m "Add the startup building blocks: the SDK client with the design's store and encryption settings, session files, recovery, paths, the data directory lock and logging"
```

---

### Task 11: Sync and the room list in the matrix adapter

Starts after Task 10 has merged. It assumes phase 0 question (1) is answered
with simplified sliding sync: the homeserver advertises
`org.matrix.simplified_msc3575`, and `RoomListService` needs it. If the
phase 0 run shows otherwise, this task is rewritten for classic sync before
it starts.

**Files:**
- Create: `crates/mxt-matrix/src/adapter.rs` (the `Matrix` handle, effect
  execution, sync state), `crates/mxt-matrix/src/rooms.rs` (room list stream,
  facts, summaries), `crates/mxt-matrix/src/bridges.rs` (one-off MSC4186
  requests and `m.bridge` parsing)
- Modify: `crates/mxt-matrix/src/lib.rs`, `crates/mxt-matrix/Cargo.toml`

**Interfaces:**
- Consumes: `RoomSummary`, `RoomName`, `RoomKind`, `UnreadCounts`,
  `NotificationMode`, `BridgeInfo`, `network_name`, `Timestamp`, `RoomId`,
  `Msg`, `SyncState`, `MatrixEffect`, `ReceiptKind` (core), `Client` and
  `Error` (Task 10).
- Produces:

```rust
/// Builds the sync service (matrix-sdk-ui `SyncService` with offline mode),
/// so startup can start it before the UI (the first login waits for the
/// device keys while it runs).
pub async fn build_sync_service(client: &Client) -> Result<Arc<SyncService>, Error>;

pub struct Matrix { /* effect channel, task handles */ }
impl Matrix {
    /// Starts the sync service if it is not running, and the tasks that turn
    /// the room list, room info updates, notification settings changes and
    /// bridge state into `Msg::RoomUpserted` / `Msg::RoomRemoved`, and the
    /// sync service state into `Msg::SyncState`.
    pub async fn start(client: Client, sync: Arc<SyncService>, receipts: ReceiptKind, to_core: UnboundedSender<Msg>) -> Result<Matrix, Error>;
    /// Runs the effect in the background. Failures arrive as `Msg::EffectFailed`.
    pub fn execute(&self, effect: MatrixEffect);
    /// Stops the tasks and the sync service.
    pub async fn stop(self);
}

// rooms.rs: the pure part, unit-tested with constructed values
pub(crate) struct RoomFacts {
    pub id: RoomId,
    pub display_name: String,
    pub is_direct: bool,
    /// Display name of the other member when the room has exactly one direct target.
    pub dm_partner_name: Option<String>,
    pub unread: UnreadCounts,
    pub mode: NotificationMode,
    pub latest: Option<Timestamp>,
    pub bridge: Option<BridgeInfo>,
    pub canonical_parent_name: Option<String>,
    /// One entry per joined parent space, `""` for unnamed ones.
    pub joined_parent_names: Vec<String>,
}
pub(crate) fn summary(facts: &RoomFacts) -> RoomSummary;

// bridges.rs
/// `content.network.displayname` and `content.protocol.displayname` of an
/// `m.bridge` or `uk.half-shot.bridge` state event's content.
pub(crate) fn parse_bridge(content: &serde_json::Value) -> BridgeInfo;
```

**Behavior:**
- Room set: `room_list_service.all_rooms()` and
  `entries_with_dynamic_adapters(page_size)` with a page size large enough
  for every room (and `add_one_page` until `maximum_number_of_rooms` is
  covered, if the SDK pages), filtered with
  `new_filter_all([new_filter_joined(), new_filter_not(new_filter_space()),
  new_filter_deduplicate_versions()])`. Keep a mirror of the room IDs by
  applying each `VectorDiff` batch. After a batch: rooms that left the mirror
  become `Msg::RoomRemoved`; rooms the batch inserted, set or reset become
  `Msg::RoomUpserted` with a fresh summary. Room info changes reach the stream
  as `Set` diffs (the SDK merges `room_info_notable_update_receiver` into it).
- Facts per room (spike notes): `display_name()` (`RoomDisplayName`),
  `is_direct()`, `direct_targets()` with `get_member_no_sync` for the partner
  name, `num_unread_messages()`, `num_unread_notifications()`,
  `num_unread_mentions()`, `is_marked_unread()`, the user-defined
  notification mode (`notification_settings().get_user_defined_room_notification_mode`;
  no room rule is `Default`, as the design says: the account default applies
  and the badge counts already follow the push rules; `AllMessages` →
  `Default`, `MentionsAndKeywordsOnly` → `MentionsAndKeywords`, `Mute` →
  `Mute`), `latest_event_timestamp()`,
  the bridge info from the adapter's cache, and parent spaces from
  `get_state_events_static::<SpaceParentEventContent>()` (the `canonical`
  flag; names of joined spaces from `client.get_room(id)` with
  `is_space()` and `display_name()`).
- `summary`: `RoomName { network: network_name(bridge, canonical_parent,
  joined_parents), room }`, where `room` is the partner's display name for a
  direct message with one target and the room's display name otherwise;
  `RoomKind::Direct` when `is_direct`.
- Notification settings changes (`notification_settings().subscribe_to_changes()`)
  recompute every room. A change to a space's name recomputes the rooms whose
  parents include it.
- Bridge state (design, "Matrix adapter"): after the room list is loaded,
  one-off `sync_events::v5` requests with `Client::send` and `conn_id`
  `mxt-bridges`: a list with `timeline_limit` 0 and `required_state`
  `[("m.bridge", "*"), ("uk.half-shot.bridge", "*")]`, paged by 100 until the
  list's `count` is covered, then `room_subscriptions` (batches of 100) for
  joined rooms no page returned. Rooms that appear later are checked with
  `room_subscriptions` once. Keep, per room, the parsed `BridgeInfo` or the
  fact that it has none; recompute and upsert the rooms whose bridge info
  arrived. The responses never reach the SDK's sync processing. Use the
  paging mode that the phase 0 `rooms` run found to work (fresh requests
  without `pos`, or `pos` chained); until that run exists, implement fresh
  requests and fall back to chaining `pos` when a later page comes back
  empty below `count`, as the spike does.
- Sync state: `SyncService::state()`: `Running` → `Synced`, `Offline` →
  `Offline`, `Error` → `Retrying`, then restart with backoff (1 s, doubling,
  at most 60 s, reset after a successful `Running`); `Idle` before the first
  start → `Starting`.
- Effects:
  - `MarkRoomRead(room)`: the room's latest event from the room list
    (`room.latest_event()`, the `Remote` value's event ID); send
    `send_multiple_receipts(Receipts::new().fully_read_marker(id)` plus
    `public_read_receipt(id)` or `private_read_receipt(id)` per `receipts`),
    then `set_unread_flag(false)` if the room is marked unread. Without a
    remote latest event, only clear the flag.
  - `SubscribeTimeline(room)`: `room_list_service.subscribe_to_rooms(&[room])`
    (phase 2 adds the `Timeline`). `UnsubscribeTimeline`: nothing until phase 2.
  - Failures: `Msg::EffectFailed { error }` with a short message (no event
    contents), and a `warn` log line.
- Logging: no message bodies; room IDs only at `debug`.

**Tests** (unit, constructed values; the SDK is not mocked):
- `summary`: a bridged Discord channel (`network` from the bridge), a Discord
  DM with only `protocol` (`Discord / Alice` from the partner name), a DM
  without a partner name (the display name), a room named through its
  canonical parent space, a room with two joined parents and no bridge (no
  network), counts and mode passed through.
- `parse_bridge`: network and protocol display names; missing `network`;
  missing `displayname`; non-object content.

- [ ] **Step 1: Write the failing tests.**
- [ ] **Step 2: Run them to see them fail** (under the shared lock).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the tests, fmt and clippy.** The adapter runs against a
  homeserver only in Task 12's check.
- [ ] **Step 5: Commit.**

```bash
git add crates/mxt-matrix Cargo.toml Cargo.lock
git commit -m "Add the sync and room list adapter, so room summaries with network names and unread counts reach the core as messages"
```

---

### Task 12: Startup flow and wiring

Connects Tasks 8–11 in `main`: the design's "Startup" steps, the matrix
adapter and the event loop, and the `M_UNKNOWN_TOKEN` handling. Starts after
Tasks 8, 9, 10 and 11 have merged.

**Files:**
- Modify: `crates/mxt/src/main.rs`, `crates/mxt/src/event_loop.rs` (only if
  the effect forwarding needs it)
- Create: `crates/mxt/src/startup.rs`

**Flow** (design, "Startup"; every failure before the terminal UI prints a
message and exits with status 1):
1. Configuration (Task 8).
2. `Paths::from_env`, `create_dirs`, `DataLock::acquire(lock_file)`.
   `LockError::Held`: print `another mxt is running (lock: <path>)`.
3. `init_logging(state, MXT_LOG)`; keep the guard alive until exit.
4. Session:
   - `load_session` returns a session:
     `build_client(&Homeserver::Saved(saved.homeserver), &paths.store_dirs())`,
     subscribe to session changes, then `restore`.
   - None: `set_aside_orphaned_store` (log it at `warn`), then
     `build_client` (with `paths.store_dirs()`) with `config.homeserver` as `Homeserver::Url`, or
     `Homeserver::Discover(<server name of the user ID>)`; print
     `Logging in as <user ID> on <homeserver>`; `prompt_secret("Password: ")`;
     `login`; `save_session` right away, before anything else can fail.
5. `build_sync_service`. Right after a first login only: start it, print
   `Waiting for this device's keys to reach the server...`,
   `wait_for_device_keys` (90 s; on timeout print that recovery may fail and
   continue), then `recovery_status`:
   - `Disabled`: print that the account has no secret storage, recovery has
     to be set up in another client, and continue.
   - otherwise: `prompt_secret("Recovery key (empty to skip): ")`; empty
     skips (print that `:recover` asks again later); `recover`; print
     `Recovery complete` or `Recovery failed: <error>` and continue either way.
6. `install_panic_hook`, `Tui::new`, the two channels, `Matrix::start(client,
   sync, config.settings.read_receipts, to_core)`, a task that forwards
   `MatrixEffect`s from the loop's channel to `Matrix::execute`, then
   `event_loop::run`.
7. On exit: `Matrix::stop`, `Tui::restore`, drop the client, release the
   lock.

**`M_UNKNOWN_TOKEN`** (design, "Startup"): subscribe to
`client.subscribe_to_session_changes()` before restoring or logging in. When
`SessionChange::UnknownToken` arrives, at any time: end the event loop (run
the loop inside `tokio::select!` with the session-change receiver), restore
the terminal, stop the adapter, drop the client so the stores are closed,
`invalidate(paths, now)`, print `The homeserver no longer accepts this
session (M_UNKNOWN_TOKEN). The old data was moved to <path>; the next start
logs in as a new device.`, and exit with status 1. The same applies when
`restore` or the first requests fail with an unknown token before the UI
starts (`Error::is_unknown_token`).

**Tests:** the flow is I/O; it is verified by running. Keep decisions that can
be pure (for example the server name from a user ID, and the homeserver
choice from the configuration and the saved session) in small functions with
unit tests.

**Startup follow-ups** (from the review of Task 10, #18):
- Restore with the saved homeserver variant of `Homeserver` (no
  `/versions` request), so mxt starts without network and shows the stored
  room list with the sync state `offline`.
- Subscribe to `subscribe_to_session_changes()` after building the client
  and before `restore`/`login`: the E2EE setup tasks send requests right
  after `restore_session`.
- A session file without a store (`data/store` deleted) is invalid: the
  crypto store would create a new Olm account for an existing device. Treat
  it like `M_UNKNOWN_TOKEN` before the UI starts (move the data aside, print
  why, exit 1).
- `prompt_secret` blocks: once the sync service runs (the recovery prompt
  after a first login), call it through `tokio::task::spawn_blocking`.
- `wait_for_device_keys` returning an error other than an unknown token is
  handled like a timeout (warn that recovery may fail, continue).

**Configuration follow-ups** (from the review of the configuration loader,
#19; same code, so they land here):
- `files.download_dir` must be absolute after `~/` expansion; `"~alice/dl"`
  and `"Downloads"` are errors at the value's position
  (`files.download_dir: "<value>" must be an absolute path or start with ~/`).
- `ActionError`s point at the offending argument: keep the argument names'
  spans in the binding DTO and report `UnknownArgument`, `WrongType`,
  `InvalidValue` and the second argument of `Conflict` at that argument's
  position (header-form bindings such as `[keymap.normal."]u"]` then point at
  the right line).
- Tests for a non-default `account.homeserver` and `ui.time_format`, and for
  a homeserver value without `https://` or `http://`.
- Delete `TableId::from_config_name` (unused; the loader maps tables through
  its DTO) and its test assertion.
- `main` reads `HOME` and `XDG_CONFIG_HOME` with `std::env::var_os`, like
  `Paths::from_env`, so a non-UTF-8 value is not silently replaced by the
  default (`config::default_path` takes `Option<OsString>` too).

- [ ] **Step 1: Write the unit tests for the pure helpers; see them fail.**
- [ ] **Step 2: Implement the flow.**
- [ ] **Step 3: Run the tests, fmt and clippy** (under the shared lock).
- [ ] **Step 4: Check by running, without a real login:**
  - `--config` pointing to a file with `user_id = "@alice:example.org"` and
    an empty `XDG_DATA_HOME`, `XDG_CACHE_HOME` and `XDG_STATE_HOME` under a
    temporary directory: mxt creates the directories (mode 0700), takes the
    lock, fails discovery for `example.org` with a readable message and
    status 1, and restores nothing because no UI started.
  - A second mxt on the same data directory while the first waits at the
    password prompt (use a reachable public homeserver in the configuration,
    for example `homeserver = "https://matrix.org"`, and do not type a
    password; quit the first with `<C-c>`): the second prints the lock
    message.
  - The orchestrator runs the check against the real account; do not log in.
- [ ] **Step 5: Commit.**

```bash
git add crates/mxt
git commit -m "Wire the startup flow, the matrix adapter and the event loop, so mxt logs in or restores its session, syncs and shows the room list"
```

---

### Task 13: Classic sync instead of sliding sync (phase 0 decision)

Phase 0 decided the sync path for the target homeserver: simplified sliding
sync lists 1 of 363 joined rooms (the bridged rooms and DMs never appear),
while classic `/sync` returns all of them. The design's "Sync path" bullet
says only the chosen path is implemented, and its classic paragraph applies:
the adapter builds the room set from `Client::rooms()` and reads `m.bridge`
from the full room state. This task replaces the sliding sync parts of
Tasks 10–12. The core is not affected. It starts after phase 2 Tasks 3–4
(the timeline adapter) have merged, because both change
`crates/mxt-matrix/src/adapter.rs`.

**Files:**
- Create: `crates/mxt-matrix/src/sync.rs` (the classic sync loop)
- Modify: `crates/mxt-matrix/src/{adapter,rooms,bridges,client,error,lib}.rs`,
  `crates/mxt/src/startup.rs`
- Delete: the one-off MSC4186 request code in `bridges.rs` (keep
  `parse_bridge` and the event precedence), the `RoomListService` and
  `SyncService` uses

**Interfaces:**

```rust
// mxt-matrix
/// The classic sync loop. Started once per process; the first login starts
/// it early so the device keys reach the server before the recovery prompt.
pub struct SyncLoop { /* task handle, state receiver */ }
impl SyncLoop {
    /// Subscribes the event cache (`client.event_cache().subscribe()`; the
    /// SDK computes unread counts and latest events from it), then runs
    /// `Client::sync_once` in a loop with `SyncToken::ReusePrevious`, a 30 s
    /// long-poll timeout, presence offline, and a filter with lazy-loaded
    /// members and a timeline limit of 10.
    pub fn start(client: &Client) -> SyncLoop;
    pub async fn stop(self);
}
// `Matrix::start` takes the `SyncLoop` instead of `Arc<SyncService>`;
// `build_sync_service` and the `SyncService` re-export are removed.
```

**Behavior:**
- Sync state: `Starting` until the first successful response; `Synced`
  after each successful response; on an error, `Offline` when the request
  did not reach the server (connection or timeout errors), `Retrying`
  otherwise, then retry with the existing backoff (1 s doubling to 60 s,
  reset after a success). `M_UNKNOWN_TOKEN` stops the loop (the session
  watch already ends mxt).
- Room set: at start, every joined room from `Client::joined_rooms()` that
  is not a space. After each sync response: rooms in `rooms.join` are
  recomputed and upserted, rooms in `rooms.leave` become `RoomRemoved`, and
  newly joined rooms are added. Room info notable updates
  (`room_info_notable_update_receiver`; read receipts and unread counts
  change through it) recompute that room; on `Lagged`, recompute every room.
  Unchanged facts are not re-sent (as now).
- Bridge info: from the stored state, `room.get_state_events(StateEventType::from("m.bridge"))`
  and `uk.half-shot.bridge`, parsed with `parse_bridge` and the existing
  precedence, recomputed when the room's state changes in a sync response.
- Facts otherwise unchanged (`RoomFacts`, `summary`, DM partner names,
  notification modes, parent spaces, latest activity via
  `latest_event_timestamp()`; classic sync registers rooms with the latest
  events subsystem once the event cache is subscribed, matrix-sdk `sync.rs`
  `subscribe_to_room_latest_events`).
- Client: `VersionBuilder::None` for every `Homeserver` variant (no sliding
  sync, no `/versions` check for it).
- Effects: `SubscribeTimeline`/`UnsubscribeTimeline` no longer touch room
  subscriptions (classic sync delivers every room); the timeline handling of
  phase 2 Task 4 stays.
- Startup (`startup.rs`): after a first login, start the `SyncLoop` before
  waiting for the device keys, as before with the sync service.

**Tests:** the sync state mapping from error kinds and the backoff (pure);
the joined/left bookkeeping from the room ID sets of a response (pure); the
bridge precedence tests stay.

- [ ] Steps: tests, implement, pass (under the lock), fmt and clippy, then
  the orchestrator runs mxt against the account (room count, network names,
  unread badges, sync state), commit
  ("Sync with classic /sync, because the target homeserver's sliding sync does not list bridged rooms").

---

### Task 14: nvim-compatible resolution of pending keys (owner decision)

The owner chose nvim's behavior over the design's original rule. Design and
code change together in one pull request.

**Design change** ("Keymap resolver", the two bullets on `Msg::Timeout` and
on a key that makes the sequence match nothing), replaced by:

- On `Msg::Timeout`, and when the next key makes the pending sequence match
  nothing: the longest binding that is a prefix of the pending keys (with the
  new key, in the second case) runs, and the keys after it are resolved
  afresh, one by one. When no prefix is a binding, only the first pending key
  is released, and the keys after it are resolved afresh. This is nvim's
  behavior.

**Code** (`crates/mxt-core/src/resolver.rs`, `crates/mxt-core/src/app.rs`):
- `Resolver::timeout(&mut self, map: &EffectiveMap, timeoutlen_ms: u64, id: TimerId) -> Vec<Resolved>`
  again: resolving the rest can start a new timer.
- A count applies to the binding that runs; the keys resolved afresh carry no
  count.
- `App` keeps the rule that keys resolved after a `Run` that changed the
  input context are resolved in the new context (#12), on the timeout path as
  well.

**Tests** (resolver and app; the first two were checked with nvim 0.12.4,
`]` and `]gq` bound):
- `]gj` in the timeline: `]` runs, then `g` is a prefix, then `gj` matches
  nothing and `g` is no binding, so `g` is released and `j` runs
  `cursor.down`.
- `]g` then the timeout: `]` runs, then `g` is resolved afresh and starts a
  new timer.
- A full miss over three keys: `<C-x><C-y>` pending as a prefix of
  `<C-x><C-y><C-z>` in `composer.insert`, then `a`: only `<C-x>` is released,
  then `<C-y>` and `a` are resolved afresh (both released, in order).
- `2]` then the timeout runs `]` with count 2; the rest carries no count.
- The existing tests that encoded the old rule are updated, and the tests
  for the old rule's special case (`a_pending_sequence_runs_only_when_it_is_a_binding_itself`)
  are replaced by the cases above.

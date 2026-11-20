# `:help`, `:map` and `:react` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three owner-approved features: `:help` lists every command with a
one-line description, generated from a command registry; `:map` lists each
keymap table's bindings after the user's configuration; `:react <emoji>`
toggles a reaction on the message under the timeline's cursor.

**Architecture:** The core gains a command registry that parsing,
completion, usage errors and `:help` all read; two help actions whose view
reuses the message view's overlay with text the core generates (the core's
open view becomes a message or a help view); the keymap keeps each table's
merged bindings and the leader for `:map`; and an optional `emoji` argument
for `message.react`, which also gains status messages for `+`. The terminal
adapter needs no new drawing: it already draws `ViewContent::Text`. No change
in `mxt-matrix`: `:react` sends the existing `ToggleReaction`.

**Tech Stack:** Rust 2024, the `emojis` crate already in `mxt-core`, edtui
(the message view's editor).

## Global Constraints

- The design is `docs/superpowers/specs/2026-11-18-mxt-design.md` ("Commands",
  "Help views", "Action registry", the message view bullet in "Terminal
  adapter and the composer", "Modes", "Layout", "Domain model"). It wins over
  this plan when they disagree.
- Everything in the repository is in English. Tests use made-up identifiers.
- `mxt-core` stays free of I/O.
- Start after PR #67 (new default bindings, `composer.leave`) is merged: it
  edits the same keymap, action list and docs tables, and its bindings
  (`<C-w>j -> composer.focus (mode = "keep")`, `]u`) are good test inputs.
- No release tag: merging is the orchestrator's, and the owner batches
  releases. Each task adds its own `CHANGELOG.md` `[Unreleased]` line;
  Task 4 adds none.
- Build only `-p mxt-core` and `-p mxt-tui` in worktree b; CI compiles the
  rest (the `mxt` crate's configuration tests included).
- Tests assert specific lines or properties, not the whole default keymap
  text (the default keymap keeps growing).

## Task 1: Command registry

**Files:** `crates/mxt-core/src/command.rs`, `crates/mxt-core/src/app/command_line.rs`.

- [ ] Replace `COMMANDS`, `takes_argument` and the `match` in `parse_command`
  with one static registry in the order of the design's Commands table: per
  command its name, its argument (`None` or `Required(usage)`; `Optional`
  comes with Task 3, so no kind is unused), a one-line description, and a
  function from the argument to the `Action` (or a `CommandError`).
- [ ] `parse_command`, the usage and argument errors, and the completion of
  command names (a space after a command that takes an argument) read the
  registry. Names match exactly; `:q` stays the only short name. Behaviour of
  the existing commands is unchanged; their error texts stay as they are.
- [ ] Tests: the argument-kind rules over the registry (a `None` command
  rejects an argument; a `Required` one without an argument gives its
  usage); a concrete completion (`re<Tab>` completes `recover` at once, the
  only candidate before Task 3; Task 3 adds the cycle `react `, `recover`,
  back to `re`); `:h` is an unknown
  command. Existing command tests keep passing.
- No CHANGELOG line (no user-visible change).

## Task 2: `:help` and `:map`

**Files:** `crates/mxt-core/src/action.rs`, `command.rs`, `app.rs` and
`app/timeline.rs` (the open view), `crates/mxt-core/src/keymap.rs`.

- [ ] Actions `help.commands` and `help.keymap` (no arguments, no default
  keys), and registry entries `:help` and `:map` (no argument).
- [ ] The core's open view becomes `Option<View>` with `View::Message(ItemRef)`
  and `View::Help` (or one variant per help view); the image checks
  (`ImageFetched`, `ImageUnavailable`) stay tied to `View::Message`. A help
  view's text goes to the tui in `ViewContent::Text`, as a message's does;
  no title, no new tui drawing. A help action while a view is open replaces
  the view. `view.close`, the `message_view` table and the `VIEW` mode work
  the same for both.
- [ ] `help_text(registry) -> String`: the exact first line from the design,
  an empty line, then one line per entry in registry order: the usage padded
  to the longest usage plus two spaces, then the description.
- [ ] `Keymap` keeps each table's bindings after the user's bindings are
  merged over its defaults (before fallback), and the leader, besides the
  effective maps the resolver uses.
- [ ] An `Action` → arguments function, the inverse of `Action::from_spec`:
  arguments at their default left out, the rest in the order `filter`,
  `pick`, `query`, `mode`, `path` (Task 3 adds `emoji`), TOML-quoted strings.
- [ ] `keymap_text(keymap) -> String`: `leader = "<key>"`, then per table in
  the design's order an empty line, `[keymap.<table>]`, and the table's own
  bindings in key-sequence order as `keys -> action` or
  `keys -> action (arguments)`; keys in the configuration's notation with
  the leader expanded; arrays joined with `; `; `nop` listed.
- [ ] Tests (core): the help text's first line exactly, one line per registry
  entry, the padding (the width taken from the registry's longest usage, not
  a fixed number, since Task 3 adds a longer one); the arguments function
  round-trips through `Action::from_spec` for every default binding; the
  keymap text after a user configuration that overrides a default, adds a
  binding with arguments, an array and a `nop` over a default, and sets
  another leader shows those lines as specified; `composer.visual` (no
  defaults) shows its header only; `:` appears under `[keymap.normal]` and
  not under `[keymap.timeline]`; `:help` and `:map` open the view, `q` and
  `<Esc>` close it, and `help.keymap` while the `:help` view is open replaces
  it (bind `help.keymap` in `message_view` for the test, or run the action
  directly: `:` cannot be typed in a view).
- [ ] CHANGELOG `### Added`: `:help` and `:map`, and the actions
  `help.commands` and `help.keymap`.

## Task 3: `:react`

**Files:** `crates/mxt-core/src/action.rs` (`MessageReact`, `ActionError`),
`crates/mxt-core/src/emoji.rs`, `command.rs`, `app/command_line.rs`,
`app/picker.rs`.

- [ ] `message.react` takes an optional `emoji`: an emoji or a shortcode with
  colons on both sides or none, resolved through `emoji.rs`. In a binding,
  an unknown or empty value is rejected when the configuration is loaded,
  with its own `ActionError` variant and message (not `InvalidValue`'s
  "must not be empty").
- [ ] The checks for both forms, in `message.react` itself: no open room →
  `No room is open`; an empty timeline → `No message to react to`; a deleted
  message → `The message is deleted` (as today). Without `emoji`: the emoji
  picker, as today. With `emoji`: `ToggleReaction` with the user's own key
  for the same emoji (by `Emoji::get`) or else `Emoji::as_str()`; move that
  choice out of `emoji_items` into one helper both use.
- [ ] The `Optional(usage)` argument kind, and the registry entry
  `:react [<emoji>]`; an unknown shortcode is the command error
  `unknown emoji "<argument as typed>"`.
- [ ] `<Tab>` after `:react ` completes shortcodes that start with the typed
  text (a leading `:` ignored, case-sensitive), without colons,
  alphabetically, cycling as `:open` does; completing the name `react` adds
  a space.
- [ ] The arguments function from Task 2 renders `emoji = "<emoji>"`.
- [ ] Tests: `👍`, `thumbsup`, `:+1:` resolve to the same emoji, `:+1` and
  `nonsense` are unknown and send nothing; no argument opens the picker;
  the user reacted `❤` (no variation selector), so `:react heart` emits
  `ToggleReaction { key: "❤" }`, and without a reaction it emits `"❤️"`;
  no room, an empty timeline and a deleted message give their status for
  `:react 👍` and for `+`; `:react` from the room list acts on the open
  room's timeline cursor; the completion candidates for `thu` and `:thu`;
  `re<Tab>` cycles `react `, `recover`, then back to `re`; a binding with
  `emoji = "nonsense"` or `emoji = ""` is rejected.
- [ ] CHANGELOG `### Added`: `:react` and `message.react`'s `emoji`
  argument; `### Changed`: `+` now says why it does nothing with no open room
  or an empty timeline.

## Task 4: Documentation

- [ ] `docs/keys.md`: the commands table (`:react`, `:help`, `:map`), the
  Commands intro (shortcode completion after `:react`), the argument
  sentence (`:help` and `:map` take none; `:react`'s is optional), that
  `:react` acts on the open room's timeline cursor while `:mute` from the
  room list acts on the room under its cursor, and "Actions without a
  default key" (`help.commands`, `help.keymap`, `message.react` with
  `emoji`).
- [ ] `docs/configuration.md`: rows for `help.commands` and `help.keymap` in
  the Actions table, `message.react`'s `emoji` argument, and shortcodes in
  the `command_line.complete` row.
- [ ] No CHANGELOG line (the tasks above add theirs).

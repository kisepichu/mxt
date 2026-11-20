# `:help`, `:map` and `:react` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three owner-approved features: `:help` lists every command with a
one-line description, generated from a command registry; `:map` lists the
effective key bindings after the user's configuration; `:react <emoji>`
toggles a reaction on the message under the timeline's cursor.

**Architecture:** The core gains a command registry that parsing,
completion, usage errors and `:help` all read; two help actions whose view
reuses the message view's overlay with text the core generates; and an
optional `emoji` argument for `message.react`. The terminal adapter only
draws the generated text in the existing overlay. No change in
`mxt-matrix`: `:react` uses the existing reaction toggle.

**Tech Stack:** Rust 2024, the `emojis` crate already in `mxt-core`, edtui
(the message view's editor).

## Global Constraints

- The design is `docs/superpowers/specs/2026-11-18-mxt-design.md` ("Commands",
  "Help views", "Action registry", the message view bullet in "Terminal
  adapter and the composer"). It wins over this plan when they disagree.
- Everything in the repository is in English. Tests use made-up identifiers.
- `mxt-core` stays free of I/O.
- No release tag: merging is the orchestrator's, and the owner batches
  releases. Each task adds its `CHANGELOG.md` `[Unreleased]` line.
- Build only `-p mxt-core` and `-p mxt-tui` in worktree b; CI compiles the
  rest (the `mxt` crate's configuration tests included).

## Task 1: Command registry

**Files:** `crates/mxt-core/src/command.rs`, `crates/mxt-core/src/app/command_line.rs`.

- [ ] Replace `COMMANDS`, `takes_argument` and the `match` in `parse_command`
  with one static registry: per command its name, its argument (`None`,
  `Required(usage)`, `Optional(usage)`), a one-line description, and a
  function from the argument to the `Action` (or a `CommandError`).
- [ ] `parse_command`, the usage and argument errors, and the completion of
  command names (with a space after a command that takes an argument) read
  the registry. Behaviour of the existing commands is unchanged.
- [ ] Tests: every registry entry parses with a valid argument and rejects a
  wrong one by its argument kind (iterate the registry, so a new entry is
  covered without a new test); name completion offers exactly the registry's
  names. Existing command tests keep passing.

## Task 2: `:help` and `:map`

**Files:** `crates/mxt-core/src/action.rs`, `command.rs`, the view state in
`crates/mxt-core/src/app/` (where the message view is held), the keymap
module (for the effective tables), `crates/mxt-tui/src/` (the message view
overlay).

- [ ] Actions `help.commands` and `help.keymap` (no arguments, no default
  keys), and registry entries `:help` and `:map` (no argument).
- [ ] The view the core holds becomes either a message or a generated text
  (a title and the text); `view.close`, the `message_view` table and the
  `VIEW` mode work the same for both.
- [ ] `help_text(registry) -> String`: the exact first line from the design,
  an empty line, then one line per entry in registry order: the usage padded
  to one column and the description.
- [ ] `keymap_text(effective keymap, leader) -> String`: `leader = <key>`,
  then per table in the design's order a header and the table's own
  bindings sorted by keys as `keys -> action (arguments)`; keys in the
  configuration's notation with the leader expanded; arguments as in the
  configuration; arrays joined with `; `; `nop` listed.
- [ ] The tui draws the generated text in the existing overlay's editor
  (read-only like a message), with the title where the message view shows
  the sender.
- [ ] Tests (core): the help text has the first line exactly and one line per
  registry entry (iterate the registry); the keymap text after a user
  configuration that overrides a default, adds a binding with arguments, an
  array, a `nop` over a default, and a different leader shows each of them
  as specified and keeps the table order and sorting; `:help` and `:map`
  open the view, `q` and `<Esc>` close it.

## Task 3: `:react`

**Files:** `crates/mxt-core/src/action.rs` (`MessageReact`),
`crates/mxt-core/src/emoji.rs`, `command.rs`, `app/command_line.rs`,
`app/picker.rs` (the reaction toggle the picker uses).

- [ ] `message.react` takes an optional `emoji` argument: an emoji or a
  shortcode with or without colons, resolved through `emoji.rs` (the
  `emojis` crate). In a binding, an unknown value is rejected when the
  configuration is loaded (`Action::from_spec`).
- [ ] Registry entry `:react [<emoji>]` (optional argument). With an
  argument it toggles the reaction on the message under the timeline's
  cursor through the same path as accepting it in the emoji picker; without
  one it opens the emoji picker for that message. An unknown shortcode is a
  status-line error `Unknown emoji: <text>`; no open room or no message
  under the cursor gives a status message and does nothing.
- [ ] `<Tab>` after `:react ` completes shortcodes that start with the typed
  text, alphabetically, cycling as `:open` does.
- [ ] Tests: `👍`, `thumbsup`, `:+1:` resolve to the same emoji; an unknown
  shortcode errors and sends nothing; no argument opens the picker; a
  reaction the user already sent is removed, another one is sent; no room
  and no message under the cursor give their status; the completion
  candidates for a prefix; a binding with `emoji = "nonsense"` is rejected.

## Task 4: Documentation

- [ ] `docs/keys.md`: the commands table (`:react`, `:help`, `:map`) and the
  actions list (`help.commands`, `help.keymap`, `message.react`'s `emoji`).
- [ ] `docs/configuration.md`: `message.react`'s `emoji` argument where
  binding arguments are described.
- [ ] `CHANGELOG.md` `[Unreleased]` `### Added`: `:help`, `:map` (with
  `help.keymap`), `:react` and the `emoji` argument.

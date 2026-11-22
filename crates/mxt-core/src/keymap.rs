//! The keymap: the default tables, the user's bindings over them, and the
//! effective map of each input context.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Bound;

use crate::action::{Action, Binding, FocusMode};
use crate::keys::{Key, KeySeq};
use crate::rooms::{Pick, RoomFilter, RoomTarget};

/// A keymap table of the configuration (design, "Tables and resolution").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TableId {
    /// The fallback for the tables that name it; only bindings that make
    /// sense everywhere outside text input.
    Normal,
    RoomList,
    Timeline,
    /// A thread is shown in place of the timeline; falls back to
    /// `timeline`, then `normal`.
    Thread,
    ComposerNormal,
    ComposerVisual,
    ComposerInsert,
    /// The mention menu is open in insert mode.
    ComposerCompletion,
    MessageView,
    Picker,
    CommandLine,
}

impl TableId {
    pub const ALL: [TableId; 11] = [
        TableId::Normal,
        TableId::RoomList,
        TableId::Timeline,
        TableId::Thread,
        TableId::ComposerNormal,
        TableId::ComposerVisual,
        TableId::ComposerInsert,
        TableId::ComposerCompletion,
        TableId::MessageView,
        TableId::Picker,
        TableId::CommandLine,
    ];

    /// "normal", "room_list", "timeline", "thread", "composer.normal",
    /// "composer.visual", "composer.insert", "composer.completion",
    /// "message_view", "picker", "command_line".
    pub fn config_name(self) -> &'static str {
        match self {
            TableId::Normal => "normal",
            TableId::RoomList => "room_list",
            TableId::Timeline => "timeline",
            TableId::Thread => "thread",
            TableId::ComposerNormal => "composer.normal",
            TableId::ComposerVisual => "composer.visual",
            TableId::ComposerInsert => "composer.insert",
            TableId::ComposerCompletion => "composer.completion",
            TableId::MessageView => "message_view",
            TableId::Picker => "picker",
            TableId::CommandLine => "command_line",
        }
    }
}

/// Where the user is typing; decides the table chain and where released keys go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputContext {
    RoomList,
    Timeline,
    /// The timeline has focus and shows a thread.
    Thread,
    ComposerNormal,
    ComposerVisual,
    ComposerInsert,
    ComposerCompletion,
    /// The message view's editor in normal or visual mode.
    MessageView,
    /// An editor in a mode where every key goes to it (search mode; the message
    /// view's editor in insert mode; a key waiting for the next one, as after
    /// `f`, an operator or `g` for `gg`).
    EditorPassthrough,
    Picker,
    CommandLine,
}

/// Where the keys the keymap did not consume go.
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
    /// In declaration order, so `context as usize` indexes arrays built from it
    /// (checked below).
    const ALL: [InputContext; 11] = [
        InputContext::RoomList,
        InputContext::Timeline,
        InputContext::Thread,
        InputContext::ComposerNormal,
        InputContext::ComposerVisual,
        InputContext::ComposerInsert,
        InputContext::ComposerCompletion,
        InputContext::MessageView,
        InputContext::EditorPassthrough,
        InputContext::Picker,
        InputContext::CommandLine,
    ];

    /// Tables consulted, earliest wins (design, "Tables and resolution").
    pub fn chain(self) -> &'static [TableId] {
        match self {
            InputContext::RoomList => &[TableId::RoomList, TableId::Normal],
            InputContext::Timeline => &[TableId::Timeline, TableId::Normal],
            InputContext::Thread => &[TableId::Thread, TableId::Timeline, TableId::Normal],
            InputContext::ComposerNormal => &[TableId::ComposerNormal, TableId::Normal],
            InputContext::ComposerVisual => &[TableId::ComposerVisual, TableId::Normal],
            InputContext::ComposerInsert => &[TableId::ComposerInsert],
            InputContext::ComposerCompletion => {
                &[TableId::ComposerCompletion, TableId::ComposerInsert]
            }
            InputContext::MessageView => &[TableId::MessageView],
            InputContext::EditorPassthrough => &[],
            InputContext::Picker => &[TableId::Picker],
            InputContext::CommandLine => &[TableId::CommandLine],
        }
    }

    pub fn sink(self) -> KeySink {
        match self {
            InputContext::RoomList | InputContext::Timeline | InputContext::Thread => KeySink::Drop,
            InputContext::ComposerNormal
            | InputContext::ComposerVisual
            | InputContext::ComposerInsert
            | InputContext::ComposerCompletion
            | InputContext::MessageView
            | InputContext::EditorPassthrough => KeySink::Editor,
            InputContext::Picker | InputContext::CommandLine => KeySink::Text,
        }
    }

    /// Whether digits start a count prefix: in the room list and the
    /// timeline, a thread's included.
    pub fn counts(self) -> bool {
        matches!(
            self,
            InputContext::RoomList | InputContext::Timeline | InputContext::Thread
        )
    }
}

const _: () = {
    let mut index = 0;
    while index < InputContext::ALL.len() {
        assert!(InputContext::ALL[index] as usize == index);
        index += 1;
    }
};

/// The tables that share the movement and search defaults.
const LISTS: &[TableId] = &[TableId::RoomList, TableId::Timeline];

/// `]u`: the topmost unread room other than the open one (design, "Default
/// keymap"); `[u`: the previous unread room, wrapping around.
const UNREAD_FIRST: Action = Action::RoomOpen(RoomTarget::Pick {
    filter: RoomFilter::Unread,
    pick: Pick::First,
});
const UNREAD_PREV: Action = Action::RoomOpen(RoomTarget::Pick {
    filter: RoomFilter::Unread,
    pick: Pick::Prev,
});

/// The default keymap (design, "Default keymap"): tables, key notation with
/// `<leader>`, and the action.
const DEFAULTS: &[(&[TableId], &str, Action)] = &[
    (&[TableId::Normal], ":", Action::CommandLineOpen),
    (
        &[TableId::Normal],
        "<leader><leader>",
        Action::PickerRooms(RoomFilter::All),
    ),
    (&[TableId::Normal], "<leader>e", Action::SidebarToggle),
    (
        &[TableId::Normal],
        "<leader>m",
        Action::PickerRooms(RoomFilter::Mentions),
    ),
    // nvim's window keys, ctrl held after `<C-w>` too. The composer is
    // below the timeline.
    (&[TableId::Normal], "<C-w>h", Action::FocusRoomList),
    (&[TableId::Normal], "<C-w><C-h>", Action::FocusRoomList),
    (&[TableId::Normal], "<C-w>l", Action::FocusTimeline),
    (&[TableId::Normal], "<C-w><C-l>", Action::FocusTimeline),
    (
        &[TableId::Normal],
        "<C-w>j",
        Action::ComposerFocus(FocusMode::Keep),
    ),
    (
        &[TableId::Normal],
        "<C-w><C-j>",
        Action::ComposerFocus(FocusMode::Keep),
    ),
    (&[TableId::Normal], "<C-w>k", Action::FocusTimeline),
    (&[TableId::Normal], "<C-w><C-k>", Action::FocusTimeline),
    (&[TableId::Normal], "]u", UNREAD_FIRST),
    (&[TableId::Normal], "[u", UNREAD_PREV),
    // nvim's jump list keys; `<C-i>` is `<Tab>`. The composer's insert mode,
    // the pickers, the command line and the message view do not fall back to
    // `normal`, so their `<Tab>` stays theirs.
    (&[TableId::Normal], "<C-o>", Action::JumpBack),
    (&[TableId::Normal], "<Tab>", Action::JumpForward),
    (LISTS, "j", Action::CursorDown),
    (LISTS, "k", Action::CursorUp),
    (LISTS, "gg", Action::CursorFirst),
    (LISTS, "G", Action::CursorLast),
    (LISTS, "<C-d>", Action::CursorHalfPageDown),
    (LISTS, "<C-u>", Action::CursorHalfPageUp),
    (LISTS, "/", Action::SearchStart),
    (LISTS, "n", Action::SearchNext),
    (LISTS, "N", Action::SearchPrev),
    (&[TableId::RoomList], "<CR>", Action::RoomOpenSelected),
    (
        &[TableId::Timeline],
        "i",
        Action::ComposerFocus(FocusMode::Insert),
    ),
    (
        &[TableId::Timeline],
        "a",
        Action::ComposerFocus(FocusMode::Insert),
    ),
    (&[TableId::Timeline], "<CR>", Action::MessageView),
    (&[TableId::Timeline], "r", Action::MessageReply),
    (&[TableId::Timeline], "e", Action::MessageEdit),
    (&[TableId::Timeline], "dd", Action::MessageDelete),
    (&[TableId::Timeline], "yy", Action::MessageYank),
    (&[TableId::Timeline], "+", Action::MessageReact(None)),
    (&[TableId::Timeline], "gx", Action::MessageOpen),
    (&[TableId::Timeline], "gd", Action::MessageGotoReply),
    (&[TableId::Timeline], "R", Action::MessageResend),
    (&[TableId::Timeline], "t", Action::ThreadOpen),
    // The main timeline binds neither, so a binding of them in `normal`
    // still applies there.
    (&[TableId::Thread], "q", Action::ThreadClose),
    (&[TableId::Thread], "<Esc>", Action::ThreadClose),
    (&[TableId::ComposerInsert], "<CR>", Action::ComposerSend),
    (&[TableId::ComposerInsert], "<C-j>", Action::ComposerNewline),
    // vim's insert-mode `<C-c>`. edtui binds no `<C-c>` in insert mode and
    // inserts no ctrl characters, so the key did nothing there before.
    (&[TableId::ComposerInsert], "<C-c>", Action::ComposerNormal),
    // vim's insert-mode `<C-w>`, and the shells' `<A-BS>`. Neither did
    // anything in insert mode before, for the same reason as `<C-c>`. The
    // window prefix `<C-w>` is a `normal` key, which insert mode never sees.
    (
        &[TableId::ComposerInsert],
        "<C-w>",
        Action::ComposerDeleteWordBefore,
    ),
    (
        &[TableId::ComposerInsert],
        "<A-BS>",
        Action::ComposerDeleteWordBefore,
    ),
    (
        &[TableId::ComposerInsert, TableId::ComposerNormal],
        "<C-x><C-e>",
        Action::ComposerExternalEditor,
    ),
    (&[TableId::ComposerNormal], "<Esc>", Action::ComposerLeave),
    (&[TableId::ComposerNormal], "<C-c>", Action::ComposerCancel),
    (
        &[TableId::ComposerCompletion],
        "<C-n>",
        Action::CompletionNext,
    ),
    (
        &[TableId::ComposerCompletion],
        "<Down>",
        Action::CompletionNext,
    ),
    (
        &[TableId::ComposerCompletion],
        "<C-p>",
        Action::CompletionPrev,
    ),
    (
        &[TableId::ComposerCompletion],
        "<Up>",
        Action::CompletionPrev,
    ),
    (
        &[TableId::ComposerCompletion],
        "<C-y>",
        Action::CompletionAccept,
    ),
    (
        &[TableId::ComposerCompletion],
        "<Tab>",
        Action::CompletionAccept,
    ),
    (
        &[TableId::ComposerCompletion],
        "<CR>",
        Action::CompletionAccept,
    ),
    (
        &[TableId::ComposerCompletion],
        "<C-e>",
        Action::CompletionCancel,
    ),
    (&[TableId::Picker], "<C-n>", Action::PickerNext),
    (&[TableId::Picker], "<Down>", Action::PickerNext),
    (&[TableId::Picker], "<Tab>", Action::PickerNext),
    (&[TableId::Picker], "<C-p>", Action::PickerPrev),
    (&[TableId::Picker], "<Up>", Action::PickerPrev),
    (&[TableId::Picker], "<S-Tab>", Action::PickerPrev),
    (&[TableId::Picker], "<CR>", Action::PickerAccept),
    (&[TableId::Picker], "<Esc>", Action::PickerCancel),
    (&[TableId::CommandLine], "<CR>", Action::CommandLineExecute),
    (&[TableId::CommandLine], "<Esc>", Action::CommandLineCancel),
    (
        &[TableId::CommandLine],
        "<Tab>",
        Action::CommandLineComplete,
    ),
    (
        &[TableId::CommandLine],
        "<Up>",
        Action::CommandLineHistoryPrev,
    ),
    (
        &[TableId::CommandLine],
        "<Down>",
        Action::CommandLineHistoryNext,
    ),
    (&[TableId::MessageView], "q", Action::ViewClose),
    // vim's `<Esc>` ends visual mode; in normal mode it closes the view.
    // mxt has no macros, so `q` closes it in visual mode too.
    (&[TableId::MessageView], "<Esc>", Action::ViewLeave),
];

/// The effective map of each input context, precomputed from the default
/// tables with the user's bindings merged over them, and those tables and
/// the leader for `:map`.
#[derive(Clone, Debug)]
pub struct Keymap {
    /// Indexed by `InputContext as usize`.
    effective: [EffectiveMap; InputContext::ALL.len()],
    /// Each table's own bindings, before fallback.
    tables: BTreeMap<TableId, BTreeMap<KeySeq, Binding>>,
    leader: KeySeq,
}

impl Keymap {
    /// The default keymap with `leader` substituted, overlaid key by key by
    /// `user`: a user binding replaces the default binding of the same table and
    /// key sequence; `nop` bindings stay in the map and shadow later tables.
    ///
    /// # Panics
    ///
    /// If `leader` is empty, because the defaults use `<leader>`. A leader
    /// from [`KeySeq::parse`] is never empty.
    pub fn new(
        leader: &KeySeq,
        user: impl IntoIterator<Item = (TableId, KeySeq, Binding)>,
    ) -> Keymap {
        let mut tables: BTreeMap<TableId, BTreeMap<KeySeq, Binding>> = BTreeMap::new();
        for (ids, text, action) in DEFAULTS {
            let keys = KeySeq::parse(text, leader)
                .unwrap_or_else(|error| panic!("default binding {text:?}: {error}"));
            for id in *ids {
                let binding = Binding::single(action.clone());
                tables.entry(*id).or_default().insert(keys.clone(), binding);
            }
        }
        for (id, keys, binding) in user {
            tables.entry(id).or_default().insert(keys, binding);
        }
        let effective = InputContext::ALL.map(|context| {
            EffectiveMap::merge(context.chain().iter().filter_map(|id| tables.get(id)))
        });
        Keymap {
            effective,
            tables,
            leader: leader.clone(),
        }
    }

    pub fn effective(&self, context: InputContext) -> &EffectiveMap {
        &self.effective[context as usize]
    }

    /// The table's own bindings, the user's merged over the defaults, before
    /// fallback, in the resolver's key order.
    fn table(&self, id: TableId) -> impl Iterator<Item = (&KeySeq, &Binding)> {
        self.tables.get(&id).into_iter().flatten()
    }
}

/// The text of `:map` (design, "Help views"): `leader = "<key>"`, then per
/// table, in [`TableId::ALL`] order, an empty line, its header
/// (`[keymap.timeline]`) and its own bindings as `keys -> action` or
/// `keys -> action (name = "value", ...)`, the actions of an array joined
/// with `; `. Keys are in the configuration's notation with the leader
/// expanded; values are TOML strings.
pub fn keymap_text(keymap: &Keymap) -> String {
    let mut text = String::from("leader = ");
    toml_string(&keymap.leader.to_string(), &mut text);
    for id in TableId::ALL {
        _ = write!(text, "\n\n[keymap.{}]", id.config_name());
        for (keys, binding) in keymap.table(id) {
            _ = write!(text, "\n{keys} -> ");
            for (index, action) in binding.actions().iter().enumerate() {
                if index > 0 {
                    text.push_str("; ");
                }
                text.push_str(action.name());
                let arguments = action.arguments();
                for (index, (name, value)) in arguments.iter().enumerate() {
                    text.push_str(if index == 0 { " (" } else { ", " });
                    text.push_str(name);
                    text.push_str(" = ");
                    toml_string(value, &mut text);
                }
                if !arguments.is_empty() {
                    text.push(')');
                }
            }
        }
    }
    text
}

/// Appends `value` as a TOML basic string: quoted, with `"`, `\` and the
/// control characters escaped.
fn toml_string(value: &str, out: &mut String) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => _ = write!(out, "\\u{:04X}", u32::from(c)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The tables of one fallback chain merged into one map.
#[derive(Clone, Debug, Default)]
pub struct EffectiveMap {
    map: BTreeMap<KeySeq, Binding>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Lookup<'a> {
    /// The binding for exactly these keys.
    pub binding: Option<&'a Binding>,
    /// Whether a longer binding starts with these keys.
    pub is_prefix: bool,
}

impl EffectiveMap {
    /// Merges the tables in order; for the same key sequence the earlier table wins.
    pub fn merge<'a>(
        tables: impl IntoIterator<Item = &'a BTreeMap<KeySeq, Binding>>,
    ) -> EffectiveMap {
        let mut map = BTreeMap::new();
        for table in tables {
            for (keys, binding) in table {
                if !map.contains_key(keys) {
                    map.insert(keys.clone(), binding.clone());
                }
            }
        }
        EffectiveMap { map }
    }

    /// The binding for exactly `keys`.
    pub fn get(&self, keys: &[Key]) -> Option<&Binding> {
        self.map.get(keys)
    }

    pub fn lookup(&self, keys: &[Key]) -> Lookup<'_> {
        // Sequences order lexicographically, so every longer sequence that
        // starts with `keys` directly follows `keys`.
        let mut after = self
            .map
            .range::<[Key], _>((Bound::Excluded(keys), Bound::Unbounded));
        Lookup {
            binding: self.get(keys),
            is_prefix: after
                .next()
                .is_some_and(|(longer, _)| longer.keys().starts_with(keys)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, ArgValue, Binding};
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
        assert_eq!(
            tl.lookup(seq("j").keys()).binding,
            Some(&one(Action::CursorDown))
        );
        assert_eq!(
            tl.lookup(seq(":").keys()).binding,
            Some(&one(Action::CommandLineOpen))
        );
        assert_eq!(
            tl.lookup(seq("<Space><Space>").keys())
                .binding
                .map(|b| b.actions()[0].name()),
            Some("picker.rooms")
        );
        let g = tl.lookup(seq("g").keys());
        assert_eq!((g.binding, g.is_prefix), (None, true));
    }

    #[test]
    fn insert_mode_does_not_see_normal_or_movement_keys() {
        let km = Keymap::new(&space(), []);
        let ins = km.effective(InputContext::ComposerInsert);
        assert_eq!(ins.lookup(seq("j").keys()).binding, None);
        assert_eq!(ins.lookup(seq(":").keys()).binding, None);
        assert_eq!(
            ins.lookup(seq("<CR>").keys()).binding,
            Some(&one(Action::ComposerSend))
        );
        let normal = km.effective(InputContext::ComposerNormal);
        assert_eq!(
            normal.lookup(seq(":").keys()).binding,
            Some(&one(Action::CommandLineOpen))
        );
        assert_eq!(normal.lookup(seq("j").keys()).binding, None);
    }

    #[test]
    fn completion_falls_back_to_insert() {
        let km = Keymap::new(&space(), []);
        let c = km.effective(InputContext::ComposerCompletion);
        assert_eq!(
            c.lookup(seq("<CR>").keys()).binding,
            Some(&one(Action::CompletionAccept))
        );
        assert_eq!(
            c.lookup(seq("<C-j>").keys()).binding,
            Some(&one(Action::ComposerNewline))
        );
    }

    #[test]
    fn ctrl_c_in_insert_mode_is_a_default_that_nop_takes_back() {
        let ctrl_c = |km: &Keymap, context| km.effective(context).get(seq("<C-c>").keys()).cloned();
        let km = Keymap::new(&space(), []);
        assert_eq!(
            ctrl_c(&km, InputContext::ComposerInsert),
            Some(one(Action::ComposerNormal))
        );
        // `<C-c>` did nothing in insert mode before; `nop` keeps that, and
        // normal mode's `<C-c>` stays.
        let nop = (TableId::ComposerInsert, seq("<C-c>"), one(Action::Nop));
        let km = Keymap::new(&space(), [nop]);
        assert_eq!(
            ctrl_c(&km, InputContext::ComposerInsert),
            Some(one(Action::Nop))
        );
        assert_eq!(
            ctrl_c(&km, InputContext::ComposerNormal),
            Some(one(Action::ComposerCancel))
        );
    }

    #[test]
    fn prefixes_are_judged_on_the_merged_map() {
        let km = Keymap::new(
            &space(),
            [
                (TableId::Timeline, seq("]"), one(Action::CursorLast)),
                (TableId::Normal, seq("]u"), one(Action::RoomMarkRead)),
            ],
        );
        let l = km.effective(InputContext::Timeline).lookup(seq("]").keys());
        assert_eq!(l.binding, Some(&one(Action::CursorLast)));
        assert!(l.is_prefix);
    }

    #[test]
    fn earlier_table_wins_and_user_overrides_defaults() {
        let km = Keymap::new(
            &space(),
            [
                (TableId::Normal, seq("j"), one(Action::AppQuit)),
                (TableId::Timeline, seq("<C-w>h"), one(Action::SidebarToggle)),
            ],
        );
        let tl = km.effective(InputContext::Timeline);
        assert_eq!(
            tl.lookup(seq("j").keys()).binding,
            Some(&one(Action::CursorDown))
        );
        assert_eq!(
            tl.lookup(seq("<C-w>h").keys()).binding,
            Some(&one(Action::SidebarToggle))
        );
        let rl = km.effective(InputContext::RoomList);
        assert_eq!(
            rl.lookup(seq("<C-w>h").keys()).binding,
            Some(&one(Action::FocusRoomList))
        );
    }

    #[test]
    fn nop_shadows_the_default() {
        let km = Keymap::new(
            &space(),
            [
                (TableId::Timeline, seq("dd"), one(Action::Nop)),
                (TableId::Timeline, seq("D"), one(Action::MessageDelete)),
            ],
        );
        let tl = km.effective(InputContext::Timeline);
        assert_eq!(tl.lookup(seq("dd").keys()).binding, Some(&one(Action::Nop)));
        assert_eq!(
            tl.lookup(seq("D").keys()).binding,
            Some(&one(Action::MessageDelete))
        );
    }

    #[test]
    fn leader_is_substituted_in_defaults() {
        let comma = KeySeq::parse(",", &KeySeq::default()).unwrap();
        let km = Keymap::new(&comma, []);
        let tl = km.effective(InputContext::Timeline);
        assert_eq!(
            tl.lookup(&[Key::char(','), Key::char('e')]).binding,
            Some(&one(Action::SidebarToggle))
        );
        assert_eq!(tl.lookup(seq("<Space>e").keys()).binding, None);
    }

    #[test]
    fn chains_and_sinks() {
        assert_eq!(InputContext::EditorPassthrough.chain(), &[] as &[TableId]);
        assert_eq!(InputContext::Picker.sink(), KeySink::Text);
        assert_eq!(InputContext::RoomList.sink(), KeySink::Drop);
        assert!(InputContext::Timeline.counts());
        assert!(!InputContext::ComposerNormal.counts());
    }

    #[test]
    fn a_thread_looks_in_thread_then_timeline_then_normal() {
        let km = Keymap::new(
            &space(),
            [
                (TableId::Normal, seq("q"), one(Action::AppQuit)),
                (TableId::Timeline, seq("x"), one(Action::MessageDelete)),
                (TableId::Thread, seq("x"), one(Action::MessageYank)),
            ],
        );
        let thread = km.effective(InputContext::Thread);
        let timeline = km.effective(InputContext::Timeline);
        let action = |map: &EffectiveMap, keys: &str| map.get(seq(keys).keys()).cloned();
        assert_eq!(action(thread, "q"), Some(one(Action::ThreadClose)));
        assert_eq!(action(thread, "<Esc>"), Some(one(Action::ThreadClose)));
        assert_eq!(action(thread, "x"), Some(one(Action::MessageYank)));
        assert_eq!(action(thread, "t"), Some(one(Action::ThreadOpen)));
        assert_eq!(action(thread, "j"), Some(one(Action::CursorDown)));
        assert_eq!(action(thread, ":"), Some(one(Action::CommandLineOpen)));
        // The main timeline binds neither `q` nor `<Esc>`.
        assert_eq!(action(timeline, "q"), Some(one(Action::AppQuit)));
        assert_eq!(action(timeline, "<Esc>"), None);
        assert_eq!(action(timeline, "x"), Some(one(Action::MessageDelete)));
        assert!(InputContext::Thread.counts());
        assert_eq!(InputContext::Thread.sink(), KeySink::Drop);
    }

    #[test]
    fn the_arguments_of_every_default_binding_build_the_same_action() {
        let km = Keymap::new(&space(), []);
        for id in TableId::ALL {
            for (keys, binding) in km.table(id) {
                for action in binding.actions() {
                    let args: Vec<(String, ArgValue)> = action
                        .arguments()
                        .into_iter()
                        .map(|(name, value)| (name.to_owned(), ArgValue::String(value.to_owned())))
                        .collect();
                    assert_eq!(
                        Action::from_spec(action.name(), &args).as_ref(),
                        Ok(action),
                        "{keys}"
                    );
                }
            }
        }
    }

    /// The lines under `[keymap.<table>]` up to the next empty line.
    fn section<'a>(text: &'a str, table: &str) -> Vec<&'a str> {
        let header = format!("[keymap.{table}]");
        let mut lines = text.lines().skip_while(|line| *line != header);
        assert_eq!(lines.next(), Some(header.as_str()), "{text}");
        lines.take_while(|line| !line.is_empty()).collect()
    }

    fn spec(name: &str, args: &[(&str, &str)]) -> Action {
        let args: Vec<(String, ArgValue)> = args
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ArgValue::String((*value).to_owned())))
            .collect();
        Action::from_spec(name, &args).unwrap()
    }

    #[test]
    fn the_keymap_text_lists_each_tables_own_bindings_after_the_users() {
        let comma = KeySeq::parse(",", &KeySeq::default()).unwrap();
        let seq = |text: &str| KeySeq::parse(text, &comma).unwrap();
        let query = spec(
            "room.open",
            &[("filter", "notify"), ("query", "say \"hi\"")],
        );
        let both = Binding::new(vec![
            Action::RoomMarkRead,
            spec("room.open", &[("filter", "unread"), ("pick", "next")]),
        ])
        .unwrap();
        let all_first = spec("room.open", &[("filter", "all"), ("pick", "first")]);
        let km = Keymap::new(
            &comma,
            [
                // Over the default `<leader>e`, `sidebar.toggle`.
                (
                    TableId::Normal,
                    seq("<leader>e"),
                    one(Action::FocusRoomList),
                ),
                (TableId::Normal, seq("<leader>q"), one(query)),
                (TableId::Timeline, seq("<leader>x"), both),
                (TableId::Timeline, seq("dd"), one(Action::Nop)),
                (TableId::Timeline, seq("gr"), one(all_first)),
                (
                    TableId::Timeline,
                    seq("<leader>t"),
                    one(spec("message.react", &[("emoji", ":+1:")])),
                ),
            ],
        );
        let text = keymap_text(&km);
        assert_eq!(text.lines().next(), Some("leader = \",\""));
        let headers: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("[keymap."))
            .collect();
        assert_eq!(
            headers,
            [
                "[keymap.normal]",
                "[keymap.room_list]",
                "[keymap.timeline]",
                "[keymap.thread]",
                "[keymap.composer.normal]",
                "[keymap.composer.visual]",
                "[keymap.composer.insert]",
                "[keymap.composer.completion]",
                "[keymap.message_view]",
                "[keymap.picker]",
                "[keymap.command_line]",
            ]
        );

        let normal = section(&text, "normal");
        for line in [
            ": -> command_line.open",
            ",, -> picker.rooms",
            ",e -> focus.room_list",
            ",m -> picker.rooms (filter = \"mentions\")",
            ",q -> room.open (filter = \"notify\", query = \"say \\\"hi\\\"\")",
            "<C-w>j -> composer.focus (mode = \"keep\")",
            "]u -> room.open (filter = \"unread\")",
            "[u -> room.open (filter = \"unread\", pick = \"prev\")",
        ] {
            assert!(normal.contains(&line), "{line}: {normal:#?}");
        }
        assert!(!text.contains("sidebar.toggle"), "{text}");

        let timeline = section(&text, "timeline");
        for line in [
            ",x -> room.mark_read; room.open (filter = \"unread\", pick = \"next\")",
            "dd -> nop",
            "gr -> room.open",
            ",t -> message.react (emoji = \"👍\")",
            "+ -> message.react",
            "i -> composer.focus",
            "j -> cursor.down",
        ] {
            assert!(timeline.contains(&line), "{line}: {timeline:#?}");
        }
        // A table shows its own bindings only.
        assert!(!timeline.iter().any(|line| line.starts_with(": ")));
        // Key order, not the text's: characters before `<CR>`, and `dd`
        // before `<C-d>` before `e`.
        let at = |keys: &str| {
            let prefix = format!("{keys} -> ");
            let at = timeline.iter().position(|line| line.starts_with(&prefix));
            at.unwrap_or_else(|| panic!("{keys}: {timeline:#?}"))
        };
        assert!(at("a") < at("<CR>"));
        assert!(at("dd") < at("<C-d>") && at("<C-d>") < at("e"));

        assert_eq!(section(&text, "composer.visual"), Vec::<&str>::new());
        let thread = section(&text, "thread");
        for line in ["q -> thread.close", "<Esc> -> thread.close"] {
            assert!(thread.contains(&line), "{line}: {thread:#?}");
        }
        assert!(timeline.contains(&"t -> thread.open"), "{timeline:#?}");
    }
}

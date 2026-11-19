//! The keymap: the default tables, the user's bindings over them, and the
//! effective map of each input context.

use std::collections::BTreeMap;
use std::ops::Bound;

use crate::action::{Action, Binding, FocusMode};
use crate::keys::{Key, KeySeq};
use crate::rooms::RoomFilter;

/// A keymap table of the configuration (design, "Tables and resolution").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TableId {
    /// The fallback for the tables that name it; only bindings that make
    /// sense everywhere outside text input.
    Normal,
    RoomList,
    Timeline,
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
    pub const ALL: [TableId; 10] = [
        TableId::Normal,
        TableId::RoomList,
        TableId::Timeline,
        TableId::ComposerNormal,
        TableId::ComposerVisual,
        TableId::ComposerInsert,
        TableId::ComposerCompletion,
        TableId::MessageView,
        TableId::Picker,
        TableId::CommandLine,
    ];

    /// "normal", "room_list", "timeline", "composer.normal", "composer.visual",
    /// "composer.insert", "composer.completion", "message_view", "picker", "command_line".
    pub fn config_name(self) -> &'static str {
        match self {
            TableId::Normal => "normal",
            TableId::RoomList => "room_list",
            TableId::Timeline => "timeline",
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
    ComposerNormal,
    ComposerVisual,
    ComposerInsert,
    ComposerCompletion,
    /// The message view's editor in normal or visual mode.
    MessageView,
    /// An editor in a mode where every key goes to it (search mode; the message
    /// view's editor in insert mode; a key waiting for its character).
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
    const ALL: [InputContext; 10] = [
        InputContext::RoomList,
        InputContext::Timeline,
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
            InputContext::RoomList | InputContext::Timeline => KeySink::Drop,
            InputContext::ComposerNormal
            | InputContext::ComposerVisual
            | InputContext::ComposerInsert
            | InputContext::ComposerCompletion
            | InputContext::MessageView
            | InputContext::EditorPassthrough => KeySink::Editor,
            InputContext::Picker | InputContext::CommandLine => KeySink::Text,
        }
    }

    /// Whether digits start a count prefix: RoomList and Timeline only.
    pub fn counts(self) -> bool {
        matches!(self, InputContext::RoomList | InputContext::Timeline)
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
    (&[TableId::Normal], "<C-w>h", Action::FocusRoomList),
    (&[TableId::Normal], "<C-w>l", Action::FocusTimeline),
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
    (&[TableId::Timeline], "+", Action::MessageReact),
    (&[TableId::Timeline], "gx", Action::MessageOpen),
    (&[TableId::Timeline], "gd", Action::MessageGotoReply),
    (&[TableId::ComposerInsert], "<CR>", Action::ComposerSend),
    (&[TableId::ComposerInsert], "<C-j>", Action::ComposerNewline),
    (
        &[TableId::ComposerInsert, TableId::ComposerNormal],
        "<C-x><C-e>",
        Action::ComposerExternalEditor,
    ),
    (&[TableId::ComposerNormal], "<Esc>", Action::FocusTimeline),
    (&[TableId::ComposerNormal], "<C-c>", Action::ComposerCancel),
    (
        &[TableId::ComposerCompletion],
        "<C-n>",
        Action::CompletionNext,
    ),
    (
        &[TableId::ComposerCompletion],
        "<C-p>",
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
    (&[TableId::Picker], "<C-p>", Action::PickerPrev),
    (&[TableId::Picker], "<CR>", Action::PickerAccept),
    (&[TableId::Picker], "<Esc>", Action::PickerCancel),
    (&[TableId::CommandLine], "<CR>", Action::CommandLineExecute),
    (&[TableId::CommandLine], "<Esc>", Action::CommandLineCancel),
    (
        &[TableId::CommandLine],
        "<Tab>",
        Action::CommandLineComplete,
    ),
    (&[TableId::MessageView], "q", Action::ViewClose),
    (&[TableId::MessageView], "<Esc>", Action::ViewClose),
];

/// The effective map of each input context, precomputed from the default
/// tables with the user's bindings merged over them.
#[derive(Clone, Debug)]
pub struct Keymap {
    /// Indexed by `InputContext as usize`.
    effective: [EffectiveMap; InputContext::ALL.len()],
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
        Keymap { effective }
    }

    pub fn effective(&self, context: InputContext) -> &EffectiveMap {
        &self.effective[context as usize]
    }
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
}

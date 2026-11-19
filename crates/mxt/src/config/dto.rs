//! The configuration file as serde reads it. Every table denies unknown
//! fields. Values that the core checks later keep their spans, so their
//! errors can name the line.

use std::collections::BTreeMap;
use std::fmt;

use mxt_core::action::ArgValue;
use mxt_core::keymap::TableId;
use mxt_core::{rooms, settings};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use toml::Spanned;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub account: Account,
    #[serde(default)]
    pub ui: Ui,
    #[serde(default)]
    pub files: Files,
    #[serde(default)]
    pub keymap: Keymap,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub user_id: Spanned<String>,
    pub homeserver: Option<Spanned<String>>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub leader: Option<Spanned<String>>,
    pub timeoutlen: Option<u64>,
    pub room_list: RoomList,
    pub time_format: Option<Spanned<String>>,
    pub image_preview: Option<ImagePreview>,
    pub read_receipts: Option<ReadReceipts>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RoomList {
    pub visible: Option<bool>,
    pub sort: Option<RoomSort>,
    /// Any integer, so every value outside the range gets the same message.
    pub width: Option<Spanned<i64>>,
    pub network: Option<NetworkPlacement>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoomSort {
    Recent,
    Name,
}

impl From<RoomSort> for rooms::RoomSort {
    fn from(sort: RoomSort) -> rooms::RoomSort {
        match sort {
            RoomSort::Recent => rooms::RoomSort::Recent,
            RoomSort::Name => rooms::RoomSort::Name,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkPlacement {
    Inline,
    Below,
}

impl From<NetworkPlacement> for mxt_tui::NetworkPlacement {
    fn from(network: NetworkPlacement) -> mxt_tui::NetworkPlacement {
        match network {
            NetworkPlacement::Inline => mxt_tui::NetworkPlacement::Inline,
            NetworkPlacement::Below => mxt_tui::NetworkPlacement::Below,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImagePreview {
    Auto,
    Off,
    Iterm2,
    Sixel,
    Kitty,
    Halfblocks,
}

impl From<ImagePreview> for settings::ImagePreview {
    fn from(preview: ImagePreview) -> settings::ImagePreview {
        use settings::ImageProtocol;
        match preview {
            ImagePreview::Auto => settings::ImagePreview::Auto,
            ImagePreview::Off => settings::ImagePreview::Off,
            ImagePreview::Iterm2 => settings::ImagePreview::Forced(ImageProtocol::Iterm2),
            ImagePreview::Sixel => settings::ImagePreview::Forced(ImageProtocol::Sixel),
            ImagePreview::Kitty => settings::ImagePreview::Forced(ImageProtocol::Kitty),
            ImagePreview::Halfblocks => settings::ImagePreview::Forced(ImageProtocol::Halfblocks),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadReceipts {
    Public,
    Private,
}

impl From<ReadReceipts> for settings::ReceiptKind {
    fn from(receipts: ReadReceipts) -> settings::ReceiptKind {
        match receipts {
            ReadReceipts::Public => settings::ReceiptKind::Public,
            ReadReceipts::Private => settings::ReceiptKind::Private,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Files {
    pub download_dir: Option<Spanned<String>>,
    pub opener: Option<Spanned<Vec<String>>>,
}

/// One keymap table: key notation and its binding.
pub type Table = BTreeMap<Spanned<String>, Spanned<Binding>>;

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Keymap {
    normal: Table,
    room_list: Table,
    timeline: Table,
    composer: Composer,
    message_view: Table,
    picker: Table,
    command_line: Table,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Composer {
    normal: Table,
    visual: Table,
    insert: Table,
    completion: Table,
}

impl Keymap {
    pub fn into_tables(self) -> [(TableId, Table); TableId::ALL.len()] {
        [
            (TableId::Normal, self.normal),
            (TableId::RoomList, self.room_list),
            (TableId::Timeline, self.timeline),
            (TableId::ComposerNormal, self.composer.normal),
            (TableId::ComposerVisual, self.composer.visual),
            (TableId::ComposerInsert, self.composer.insert),
            (TableId::ComposerCompletion, self.composer.completion),
            (TableId::MessageView, self.message_view),
            (TableId::Picker, self.picker),
            (TableId::CommandLine, self.command_line),
        ]
    }
}

/// A binding value: an action name, a table with `action` and the action's
/// arguments, or an array of those. An empty array is left to the core's
/// `Binding::new` to reject.
pub enum Binding {
    /// A name or a table; its span is the binding's.
    One(ActionSpec),
    Many(Vec<Spanned<ActionSpec>>),
}

/// An action as written, before `Action::from_spec` checks it.
pub struct ActionSpec {
    pub name: String,
    pub args: Vec<(String, ArgValue)>,
    /// Where each argument's name starts in the file, in the order of `args`.
    pub arg_starts: Vec<usize>,
}

impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Binding, D::Error> {
        struct BindingVisitor;

        impl<'de> Visitor<'de> for BindingVisitor {
            type Value = Binding;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                ActionVisitor.expecting(f)?;
                f.write_str(", or an array of them")
            }

            fn visit_str<E: de::Error>(self, name: &str) -> Result<Binding, E> {
                ActionVisitor.visit_str(name).map(Binding::One)
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Binding, A::Error> {
                ActionVisitor.visit_map(map).map(Binding::One)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Binding, A::Error> {
                let mut actions = Vec::new();
                while let Some(action) = seq.next_element()? {
                    actions.push(action);
                }
                Ok(Binding::Many(actions))
            }
        }

        deserializer.deserialize_any(BindingVisitor)
    }
}

impl<'de> Deserialize<'de> for ActionSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<ActionSpec, D::Error> {
        deserializer.deserialize_any(ActionVisitor)
    }
}

struct ActionVisitor;

impl<'de> Visitor<'de> for ActionVisitor {
    type Value = ActionSpec;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an action name or a table with \"action\"")
    }

    fn visit_str<E: de::Error>(self, name: &str) -> Result<ActionSpec, E> {
        Ok(ActionSpec {
            name: name.to_owned(),
            args: Vec::new(),
            arg_starts: Vec::new(),
        })
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ActionSpec, A::Error> {
        let mut name = None;
        let mut args = Vec::new();
        let mut arg_starts = Vec::new();
        while let Some(key) = map.next_key::<Spanned<String>>()? {
            if key.get_ref() == "action" {
                name = Some(map.next_value()?);
            } else {
                let Arg(value) = map.next_value()?;
                arg_starts.push(key.span().start);
                args.push((key.into_inner(), value));
            }
        }
        let name = name.ok_or_else(|| de::Error::missing_field("action"))?;
        Ok(ActionSpec {
            name,
            args,
            arg_starts,
        })
    }
}

/// An action argument: a string, an integer or a boolean.
struct Arg(ArgValue);

impl<'de> Deserialize<'de> for Arg {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Arg, D::Error> {
        struct ArgVisitor;

        impl Visitor<'_> for ArgVisitor {
            type Value = ArgValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string, an integer or a boolean")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<ArgValue, E> {
                Ok(ArgValue::String(value.to_owned()))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<ArgValue, E> {
                Ok(ArgValue::String(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<ArgValue, E> {
                Ok(ArgValue::Integer(value))
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<ArgValue, E> {
                Ok(ArgValue::Boolean(value))
            }
        }

        deserializer.deserialize_any(ArgVisitor).map(Arg)
    }
}

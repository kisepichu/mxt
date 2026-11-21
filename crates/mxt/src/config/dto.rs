//! The configuration file as serde reads it, once the loader has removed the
//! keys and tables it does not know. Every table denies unknown fields
//! anyway, so a key the loader knows and these structs do not is an error.
//! Values that the loader checks later keep their spans, so their errors and
//! warnings can name the line.

use std::collections::BTreeMap;
use std::fmt;

use mxt_core::action::ArgValue;
use mxt_core::keymap::TableId;
use mxt_core::settings;
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
    /// One of [`IMAGE_PREVIEWS`].
    pub image_preview: Option<Spanned<String>>,
    /// One of [`READ_RECEIPTS`].
    pub read_receipts: Option<Spanned<String>>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RoomList {
    pub visible: Option<bool>,
    /// A comma-separated list of `SortKey::config_name()`s, which the loader
    /// splits (design, "Layers").
    pub sort: Option<Spanned<String>>,
    /// Any integer, so every value outside the range gets the same message.
    pub width: Option<Spanned<i64>>,
    /// One of [`NETWORK_PLACEMENTS`].
    pub network: Option<Spanned<String>>,
}

// The values of the settings with fixed values. The settings are strings, so
// the loader can ignore a value a newer version knows.

/// The values of `ui.room_list.network`.
pub const NETWORK_PLACEMENTS: &[(&str, mxt_tui::NetworkPlacement)] = &[
    ("inline", mxt_tui::NetworkPlacement::Inline),
    ("below", mxt_tui::NetworkPlacement::Below),
];

/// The values of `ui.image_preview`.
pub const IMAGE_PREVIEWS: &[(&str, settings::ImagePreview)] = {
    use settings::{ImagePreview, ImageProtocol};
    &[
        ("auto", ImagePreview::Auto),
        ("off", ImagePreview::Off),
        ("iterm2", ImagePreview::Forced(ImageProtocol::Iterm2)),
        ("sixel", ImagePreview::Forced(ImageProtocol::Sixel)),
        ("kitty", ImagePreview::Forced(ImageProtocol::Kitty)),
        (
            "halfblocks",
            ImagePreview::Forced(ImageProtocol::Halfblocks),
        ),
    ]
};

/// The values of `ui.read_receipts`.
pub const READ_RECEIPTS: &[(&str, settings::ReceiptKind)] = &[
    ("public", settings::ReceiptKind::Public),
    ("private", settings::ReceiptKind::Private),
];

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

/// An action argument: a string, an integer, a boolean, or
/// [`ArgValue::Unsupported`] for any other TOML value, so that
/// `Action::from_spec` decides about every argument whatever its value.
struct Arg(ArgValue);

impl<'de> Deserialize<'de> for Arg {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Arg, D::Error> {
        struct ArgVisitor;

        impl<'de> Visitor<'de> for ArgVisitor {
            type Value = ArgValue;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a TOML value")
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

            fn visit_f64<E: de::Error>(self, _: f64) -> Result<ArgValue, E> {
                Ok(ArgValue::Unsupported)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<ArgValue, A::Error> {
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(ArgValue::Unsupported)
            }

            /// A table, or a date-time, which toml hands over as a map.
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ArgValue, A::Error> {
                while map
                    .next_entry::<de::IgnoredAny, de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(ArgValue::Unsupported)
            }
        }

        deserializer.deserialize_any(ArgVisitor).map(Arg)
    }
}

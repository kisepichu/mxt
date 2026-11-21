//! The configuration file (design, "Configuration"): where it is, and how it
//! becomes checked core types. A mistake is an error with its line and column
//! before the terminal UI starts; a name this version does not know is a
//! warning that ignores its setting or binding (design, "Layers"). Nothing is
//! silently ignored.

mod dto;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::{self, Write as _};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use mxt_core::action::{Action, ActionError, Binding};
use mxt_core::ids::UserId;
use mxt_core::keymap::{Keymap, TableId};
use mxt_core::keys::{Key, KeyCode, KeyParseError, KeySeq, Modifiers};
use mxt_core::settings::{ReceiptKind, Settings};
use mxt_tui::{RoomListLayout, RoomListWidth, TimeFormat};
use serde::Deserialize as _;
use toml::Spanned;
use toml::de::{DeTable, DeValue};
use tracing::warn;

use crate::paths;

/// The configuration with every value checked and every default applied.
#[derive(Debug)]
pub struct Config {
    pub user_id: UserId,
    /// `None`: discovered from the user ID's server name via .well-known.
    pub homeserver: Option<String>,
    pub keymap: Keymap,
    pub settings: Settings,
    /// Applied in the local time zone by `mxt-tui`.
    pub time_format: TimeFormat,
    /// `ui.room_list.width` and `ui.room_list.network`, for `mxt-tui`.
    pub room_list_layout: RoomListLayout,
    /// Absolute.
    pub download_dir: PathBuf,
    pub opener: Vec<String>,
    /// The settings and bindings that were ignored, in the order of the file.
    pub warnings: Vec<ConfigWarning>,
}

/// Printed with the path when the file does not exist.
pub const MINIMAL_EXAMPLE: &str = "[account]\nuser_id = \"@alice:example.org\"\n";

const DEFAULT_DOWNLOAD_DIR: &str = "~/Downloads";
const DEFAULT_OPENER: &str = if cfg!(target_os = "macos") {
    "open"
} else {
    "xdg-open"
};

/// `$XDG_CONFIG_HOME/mxt/config.toml`, or `$HOME/.config/mxt/config.toml` when
/// `XDG_CONFIG_HOME` is unset, empty or relative (the same on macOS). `env` is
/// `std::env::var_os`, like for `Paths::from_env`.
pub fn default_path(env: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    paths::base_dir(env, "XDG_CONFIG_HOME", ".config").map(|base| base.join("mxt/config.toml"))
}

/// Reads and converts the file. `home` expands a leading `~` in paths.
pub fn load(path: &Path, home: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ConfigError::NotFound {
            path: path.to_owned(),
        },
        _ => ConfigError::Read {
            path: path.to_owned(),
            error,
        },
    })?;
    parse(&text, path, home)
}

/// The same for a string, for tests; `path` is only used in messages.
pub fn parse(text: &str, path: &Path, home: &Path) -> Result<Config, ConfigError> {
    let source = Source { text, path };
    let mut document = DeTable::parse(text).map_err(|error| source.syntax_error(&error))?;
    let mut warnings = Vec::new();
    prune(
        &source,
        document.get_mut(),
        Node::Root,
        &mut String::new(),
        &mut warnings,
    );
    let root = document.span();
    let dto::File {
        account,
        ui,
        files,
        keymap,
    } = match dto::File::deserialize(toml::de::Deserializer::from(document)) {
        Ok(file) => file,
        // A key that is missing may be there misspelled, which its warning
        // names (design, "Layers").
        Err(error) => {
            warnings.sort_by_key(|warning| (warning.line, warning.column));
            return Err(source.dto_error(&error, &root, warnings));
        }
    };

    let user_id = account.user_id;
    if !is_user_id(user_id.get_ref()) {
        return Err(source.invalid(
            &user_id,
            format!(
                "account.user_id: {:?} is not a Matrix user ID such as \"@alice:example.org\"",
                user_id.get_ref()
            ),
        ));
    }
    if let Some(url) = &account.homeserver
        && !is_http_url(url.get_ref())
    {
        return Err(source.invalid(
            url,
            format!(
                "account.homeserver: {:?} is not an https:// or http:// URL",
                url.get_ref()
            ),
        ));
    }

    let (leader, leader_fell_back) = match &ui.leader {
        None => (default_leader(), false),
        Some(text) => match KeySeq::parse(text.get_ref(), &KeySeq::default()) {
            Ok(leader) => (leader, false),
            Err(error) if error.is_unknown_name() => {
                warnings.push(source.ignored(text, key_error("ui.leader", text, &error)));
                (default_leader(), true)
            }
            Err(error) => return Err(source.invalid(text, key_error("ui.leader", text, &error))),
        },
    };
    // The leader as mxt 0.1.2 and earlier read it, with `<C-i>` apart from
    // `<Tab>`, for the check of the same keys spelled twice.
    let leader_apart = match &ui.leader {
        Some(text) if !leader_fell_back => {
            KeySeq::parse_ctrl_i_apart(text.get_ref(), &KeySeq::default())
                .unwrap_or_else(|_| leader.clone())
        }
        _ => leader.clone(),
    };
    let defaults = Settings::default();
    let read_receipts = match ui.read_receipts {
        None => defaults.read_receipts,
        // Private, not the default: receipts go out when a room opens, and
        // opening one takes a key, which clears the status line, so public
        // receipts could go out before the warning is seen (design, "Layers").
        Some(value) => named(dto::READ_RECEIPTS, value.get_ref()).unwrap_or_else(|| {
            let message = format!(
                "ui.read_receipts: unknown value {:?}, using \"private\"",
                value.get_ref()
            );
            warnings.push(source.warning_at(value.span().start, message));
            ReceiptKind::Private
        }),
    };
    let settings = Settings {
        timeoutlen_ms: ui.timeoutlen.unwrap_or(defaults.timeoutlen_ms),
        room_sort: choice(
            &source,
            "ui.room_list.sort",
            ui.room_list.sort,
            dto::ROOM_SORTS,
            &mut warnings,
        )
        .unwrap_or(defaults.room_sort),
        room_list_visible: ui.room_list.visible.unwrap_or(defaults.room_list_visible),
        image_preview: choice(
            &source,
            "ui.image_preview",
            ui.image_preview,
            dto::IMAGE_PREVIEWS,
            &mut warnings,
        )
        .unwrap_or(defaults.image_preview),
        read_receipts,
    };
    let time_format = match ui.time_format {
        Some(format) if format.get_ref().is_empty() => {
            return Err(source.invalid(&format, "ui.time_format: must not be empty".into()));
        }
        // jiff may accept more directives in a compatible release, and its
        // errors do not tell an unknown directive from a malformed one, so a
        // format it rejects is a warning (design, "Layers").
        Some(format) => {
            let at = format.span().start;
            TimeFormat::parse(format.into_inner()).unwrap_or_else(|error| {
                warnings.push(source.ignored_at(at, format!("ui.time_format: {error}")));
                TimeFormat::default()
            })
        }
        None => TimeFormat::default(),
    };
    let room_list_width = match ui.room_list.width {
        Some(width) => RoomListWidth::new(*width.get_ref()).ok_or_else(|| {
            let (min, max) = RoomListWidth::RANGE.into_inner();
            source.invalid(
                &width,
                format!("ui.room_list.width: must be from {min} to {max}"),
            )
        })?,
        None => RoomListWidth::default(),
    };
    let room_list_layout = RoomListLayout {
        width: room_list_width,
        network: choice(
            &source,
            "ui.room_list.network",
            ui.room_list.network,
            dto::NETWORK_PLACEMENTS,
            &mut warnings,
        )
        .unwrap_or_default(),
    };

    let download_dir = match files.download_dir {
        Some(dir) => {
            let path = expand_home(dir.get_ref(), home);
            if !path.is_absolute() {
                let message = format!(
                    "files.download_dir: {:?} must be an absolute path or start with ~/",
                    dir.get_ref()
                );
                return Err(source.invalid(&dir, message));
            }
            path
        }
        None => expand_home(DEFAULT_DOWNLOAD_DIR, home),
    };
    let opener = match files.opener {
        Some(opener) if opener.get_ref().first().is_none_or(String::is_empty) => {
            return Err(source.invalid(
                &opener,
                "files.opener: must start with a program, for example [\"xdg-open\"]".into(),
            ));
        }
        Some(opener) => opener.into_inner(),
        None => vec![DEFAULT_OPENER.to_owned()],
    };

    let leaders = Leaders {
        leader: &leader,
        fell_back: leader_fell_back,
        apart: &leader_apart,
    };
    let bindings = bindings(&source, keymap, &leaders, &mut warnings)?;
    warnings.sort_by_key(|warning| (warning.line, warning.column));
    Ok(Config {
        user_id: UserId::new(user_id.into_inner()),
        homeserver: account.homeserver.map(Spanned::into_inner),
        keymap: Keymap::new(&leader, bindings),
        settings,
        time_format,
        room_list_layout,
        download_dir,
        opener,
        warnings,
    })
}

/// Logs each warning at `warn` with the target `mxt::config`, once logging
/// has started (design, "Layers").
pub fn log_warnings(warnings: &[ConfigWarning]) {
    for warning in warnings {
        warn!("{warning}");
    }
}

/// `<Space>`, the default of `ui.leader`.
fn default_leader() -> KeySeq {
    KeySeq::from_keys(vec![Key::char(' ')])
}

/// A setting with fixed values, `names` naming each: `None` when it is not
/// set or, with a warning, when its value is unknown, so its default applies.
fn choice<T: Copy>(
    source: &Source<'_>,
    setting: &str,
    value: Option<Spanned<String>>,
    names: &[(&str, T)],
    warnings: &mut Vec<ConfigWarning>,
) -> Option<T> {
    let value = value?;
    let known = named(names, value.get_ref());
    if known.is_none() {
        let mut message = format!("{setting}: unknown value {:?}", value.get_ref());
        for (index, (name, _)) in names.iter().enumerate() {
            let separator = if index == 0 { "; expected one of" } else { "," };
            _ = write!(message, "{separator} {name:?}");
        }
        warnings.push(source.ignored(&value, message));
    }
    known
}

fn named<T: Copy>(names: &[(&str, T)], value: &str) -> Option<T> {
    names
        .iter()
        .find(|(name, _)| *name == value)
        .map(|&(_, known)| known)
}

#[derive(Debug)]
pub enum ConfigError {
    NotFound {
        path: PathBuf,
    },
    Read {
        path: PathBuf,
        error: io::Error,
    },
    /// `line` and `column` are 1-based; `column` counts characters.
    Invalid {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
        /// The warnings found before a wrong type or a missing key stopped
        /// the reading, printed after the error: a key that is missing may
        /// be one of them, misspelled. Empty for any other error.
        warnings: Vec<ConfigWarning>,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotFound { path } => write!(f, "{}: no such file", path.display()),
            ConfigError::Read { path, error } => write!(f, "{}: {error}", path.display()),
            ConfigError::Invalid {
                path,
                line,
                column,
                message,
                warnings,
            } => {
                write!(f, "{}:{line}:{column}: {message}", path.display())?;
                warnings
                    .iter()
                    .try_for_each(|warning| write!(f, "\n{warning}"))
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Read { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// A setting or binding that was ignored because it names something this
/// version does not know (design, "Layers").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigWarning {
    pub path: PathBuf,
    /// 1-based, as for `ConfigError::Invalid`; `column` counts characters.
    pub line: usize,
    pub column: usize,
    /// The setting and the reason, ending in `(ignored)`, or for
    /// `ui.read_receipts` what applies instead.
    pub message: String,
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ConfigWarning {
            path,
            line,
            column,
            message,
        } = self;
        write!(f, "{}:{line}:{column}: {message}", path.display())
    }
}

/// The leader the bindings are read with.
struct Leaders<'a> {
    leader: &'a KeySeq,
    /// Whether `ui.leader` was ignored and `leader` is `<Space>`.
    fell_back: bool,
    /// `leader` as mxt 0.1.2 and earlier read it, with `<C-i>` apart from
    /// `<Tab>`.
    apart: &'a KeySeq,
}

/// The user's bindings of every table, checked in the order of the design's
/// "Layers": the keys, the same keys spelled twice in the table, then the
/// actions. A binding with a key name or an action's name this version does
/// not know is left out with a warning, so the default binding of its keys
/// stays. Two spellings of one key sequence in a table (`<leader>x` and
/// `<Space>x`, `<C-a>` and `<C-A>`) are an error at the second, because
/// `Keymap::new` would keep only one of them. Two exceptions are warnings
/// that ignore one binding, because the configuration loaded before:
///
/// - when `ui.leader` fell back to `<Space>` and only the fallback makes
///   them one (`<leader>x` and `<Space>x`, `<leader>x<Space>` and
///   `<Space>x<leader>`), a version that knows the leader tells them apart,
///   so the second is ignored;
/// - when only `<C-i>` being another name for `<Tab>` makes them one
///   (`<C-i>` and `<Tab>`), which mxt 0.1.2 and earlier told apart, the one
///   that writes `<C-i>` is ignored, whichever comes first: only the
///   `<Tab>` one could ever fire.
fn bindings(
    source: &Source<'_>,
    keymap: dto::Keymap,
    leaders: &Leaders<'_>,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<Vec<(TableId, KeySeq, Binding)>, ConfigError> {
    let mut bindings = Vec::new();
    for (id, table) in keymap.into_tables() {
        let setting = format!("keymap.{}", id.config_name());
        let mut entries: Vec<_> = table.into_iter().collect();
        entries.sort_by_key(|(key, _)| key.span().start);
        // Each key sequence's spelling and binding, `None` when ignored.
        let mut seen: BTreeMap<KeySeq, (Spanned<String>, Option<Binding>)> = BTreeMap::new();
        // The spelling of each key sequence as mxt 0.1.2 and earlier read it.
        let mut seen_apart: BTreeMap<KeySeq, Spanned<String>> = BTreeMap::new();
        for (key, value) in entries {
            let keys = match KeySeq::parse(key.get_ref(), leaders.leader) {
                Ok(keys) => keys,
                Err(error) if error.is_unknown_name() => {
                    warnings.push(source.ignored(&key, key_error(&setting, &key, &error)));
                    continue;
                }
                Err(error) => return Err(source.invalid(&key, key_error(&setting, &key, &error))),
            };
            // It parses whenever `parse` does: only the keys it gives differ.
            let keys_apart = KeySeq::parse_ctrl_i_apart(key.get_ref(), leaders.apart)
                .unwrap_or_else(|_| keys.clone());
            if let Some(first) = seen_apart.get(&keys_apart) {
                let message = same_keys(source, &setting, &key, first);
                if leaders.fell_back && differ_by_leader(key.get_ref(), first.get_ref()) {
                    let message = format!("{message}, since ui.leader fell back to \"<Space>\"");
                    warnings.push(source.ignored(&key, message));
                    continue;
                }
                return Err(source.invalid(&key, message));
            }
            if let Some((first, first_binding)) = seen.get(&keys) {
                // Only the `<C-i>` alias makes them one.
                let writes = |text: &Spanned<String>| writes_ctrl_i(text.get_ref(), leaders.apart);
                if !writes(first) || writes(&key) {
                    let message = format!(
                        "{}{CTRL_I_IS_TAB}",
                        same_keys(source, &setting, &key, first)
                    );
                    warnings.push(source.ignored(&key, message));
                    seen_apart.insert(keys_apart, key);
                    continue;
                }
                // An earlier binding ignored for its action is not counted
                // twice.
                if first_binding.is_some() {
                    let message = format!(
                        "{}{CTRL_I_IS_TAB}",
                        same_keys(source, &setting, first, &key)
                    );
                    warnings.push(source.ignored(first, message));
                }
            }
            let binding = binding(
                source,
                &format!("{setting}.{}", key_segment(key.get_ref())),
                value,
                warnings,
            )?;
            // An ignored binding still takes its keys: a version that knows
            // its action rejects a second spelling of them too.
            seen_apart.insert(keys_apart, key.clone());
            seen.insert(keys, (key, binding));
        }
        bindings.extend(
            seen.into_iter()
                .filter_map(|(keys, (_, binding))| Some((id, keys, binding?))),
        );
    }
    Ok(bindings)
}

/// The end of the warning about a spelling with `<C-i>` that another
/// spelling with `<Tab>` duplicates.
const CTRL_I_IS_TAB: &str = ", since <C-i> is another name for <Tab>";

/// `<setting>: "<key>" is the same key sequence as "<other>" on line <n>`.
fn same_keys(
    source: &Source<'_>,
    setting: &str,
    key: &Spanned<String>,
    other: &Spanned<String>,
) -> String {
    format!(
        "{setting}: {:?} is the same key sequence as {:?} on line {}",
        key.get_ref(),
        other.get_ref(),
        source.position(other.span().start).0,
    )
}

/// Whether two spellings that parse are different key sequences with a
/// leader that no other spelling writes, as a leader this version does not
/// know: `<F13>` is not key notation, so only `<leader>` gives it.
fn differ_by_leader(a: &str, b: &str) -> bool {
    let leader = KeySeq::from_keys(vec![Key::new(KeyCode::F(13), Modifiers::default())]);
    KeySeq::parse(a, &leader) != KeySeq::parse(b, &leader)
}

/// Whether a spelling that parses has ctrl and `i` in it, as mxt 0.1.2 and
/// earlier read it: a key that never arrives, since terminals send `<Tab>`.
fn writes_ctrl_i(text: &str, leader_apart: &KeySeq) -> bool {
    KeySeq::parse_ctrl_i_apart(text, leader_apart).is_ok_and(|keys| {
        keys.keys()
            .iter()
            .any(|key| key.code() == KeyCode::Char('i') && key.mods().ctrl)
    })
}

/// Checks the actions of one binding in order; `setting` names it in
/// messages. The first error decides: one about a name this version does
/// not know ignores the whole binding with a warning (`None`), any other
/// is an error.
fn binding(
    source: &Source<'_>,
    setting: &str,
    value: Spanned<dto::Binding>,
    warnings: &mut Vec<ConfigWarning>,
) -> Result<Option<Binding>, ConfigError> {
    let span = value.span();
    let specs = match value.into_inner() {
        dto::Binding::One(spec) => vec![Spanned::new(span.clone(), spec)],
        dto::Binding::Many(specs) => specs,
    };
    let mut actions = Vec::with_capacity(specs.len());
    for spec in specs {
        let spec_ref = spec.get_ref();
        match Action::from_spec(&spec_ref.name, &spec_ref.args) {
            Ok(action) => actions.push(action),
            Err(error) => {
                let at = argument_start(spec_ref, &error).unwrap_or(spec.span().start);
                let message = format!("{setting}: {error}");
                if error.is_unknown_name() {
                    warnings.push(source.ignored_at(at, message));
                    return Ok(None);
                }
                return Err(source.invalid_at(at, message));
            }
        }
    }
    let binding = Binding::new(actions).ok_or_else(|| {
        let message =
            format!("{setting}: an empty array binds nothing; write \"nop\" to remove a binding");
        source.invalid_at(span.start, message)
    })?;
    Ok(Some(binding))
}

/// Where the name of the argument that `error` is about starts, so a binding
/// written as a `[header]` table gets the argument's line. For a conflict,
/// the later of the two arguments in the file.
fn argument_start(spec: &dto::ActionSpec, error: &ActionError) -> Option<usize> {
    let start = |arg: &str| {
        let index = spec.args.iter().position(|(name, _)| name == arg)?;
        spec.arg_starts.get(index).copied()
    };
    match error {
        ActionError::UnknownArgument { arg, .. } => start(arg),
        ActionError::WrongType { arg, .. }
        | ActionError::UnknownValue { arg, .. }
        | ActionError::Empty { arg, .. } => start(arg),
        ActionError::UnknownEmoji { .. } => start("emoji"),
        ActionError::Conflict {
            args: [first, second],
            ..
        } => start(first).max(start(second)),
        ActionError::UnknownAction { .. } | ActionError::MissingArgument { .. } => None,
    }
}

fn key_error(setting: &str, text: &Spanned<String>, error: &KeyParseError) -> String {
    format!("{setting}: invalid key {:?}: {error}", text.get_ref())
}

/// `@localpart:server` with both parts non-empty and no whitespace. The SDK
/// checks the full grammar when it logs in.
fn is_user_id(id: &str) -> bool {
    !id.contains(char::is_whitespace)
        && id
            .strip_prefix('@')
            .and_then(|rest| rest.split_once(':'))
            .is_some_and(|(local, server)| !local.is_empty() && !server.is_empty())
}

fn is_http_url(url: &str) -> bool {
    ["https://", "http://"].iter().any(|scheme| {
        url.strip_prefix(scheme)
            .is_some_and(|rest| !rest.is_empty())
    })
}

/// `~` and a leading `~/` stand for `home`; other paths are kept as written.
fn expand_home(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home.to_owned(),
        Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

/// A key as a dotted TOML path writes it: bare when it can be.
fn key_segment(key: &str) -> Cow<'_, str> {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        Cow::Borrowed(key)
    } else {
        Cow::Owned(format!("{key:?}"))
    }
}

/// The text being loaded, for turning byte offsets into positions.
struct Source<'a> {
    text: &'a str,
    path: &'a Path,
}

impl Source<'_> {
    fn invalid<T>(&self, at: &Spanned<T>, message: String) -> ConfigError {
        self.invalid_at(at.span().start, message)
    }

    fn invalid_at(&self, at: usize, message: String) -> ConfigError {
        self.invalid_with(at, message, Vec::new())
    }

    fn invalid_with(
        &self,
        at: usize,
        message: String,
        warnings: Vec<ConfigWarning>,
    ) -> ConfigError {
        let (line, column) = self.position(at);
        ConfigError::Invalid {
            path: self.path.to_owned(),
            line,
            column,
            message,
            warnings,
        }
    }

    /// A warning that the setting or binding is ignored, so its default
    /// applies.
    fn ignored<T>(&self, at: &Spanned<T>, message: String) -> ConfigWarning {
        self.ignored_at(at.span().start, message)
    }

    fn ignored_at(&self, at: usize, message: String) -> ConfigWarning {
        self.warning_at(at, format!("{message} (ignored)"))
    }

    fn warning_at(&self, at: usize, message: String) -> ConfigWarning {
        let (line, column) = self.position(at);
        ConfigWarning {
            path: self.path.to_owned(),
            line,
            column,
            message,
        }
    }

    /// The 1-based line and column (in characters) of byte `at`.
    fn position(&self, at: usize) -> (usize, usize) {
        let before = &self.text[..self.text.floor_char_boundary(at)];
        let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
        let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
        (line, before[line_start..].chars().count() + 1)
    }

    /// A TOML syntax error; it names no setting, as the document does not
    /// parse.
    fn syntax_error(&self, error: &toml::de::Error) -> ConfigError {
        let at = error.span().map_or(0, |span| span.start);
        self.invalid_at(at, error.message().to_owned())
    }

    /// A serde error of the DTOs (wrong types, missing keys) with the
    /// `warnings` found before it, prefixed with the setting it is in. An
    /// error with `root`, the span of the document, is about the document
    /// itself (a missing `account`), so it names no setting, not even the
    /// table its position falls in.
    fn dto_error(
        &self,
        error: &toml::de::Error,
        root: &Range<usize>,
        warnings: Vec<ConfigWarning>,
    ) -> ConfigError {
        let at = error.span().map_or(0, |span| span.start);
        let setting = DeTable::parse(self.text)
            .ok()
            .filter(|_| error.span().as_ref() != Some(root))
            .and_then(|document| path_in_table(document.get_ref(), at).map(|path| dotted(&path)))
            .filter(|setting| !setting.is_empty());
        let message = match setting {
            Some(setting) => format!("{setting}: {}", error.message()),
            None => error.message().to_owned(),
        };
        self.invalid_with(at, message, warnings)
    }
}

/// A key of the document that this version knows (design, "Layers").
#[derive(Clone, Copy)]
enum Node {
    Root,
    Account,
    Ui,
    RoomList,
    Files,
    /// `keymap` (`""`), or a table inside it that holds keymap tables, by
    /// its dotted name below `keymap` (`"composer"`).
    Keymap(&'static str),
    /// A keymap table: its keys are bindings, which [`bindings`] checks.
    Bindings,
    /// A setting, whose value the DTOs check.
    Setting,
}

impl Node {
    /// What `key` is in this table; `None` for a key this version does not
    /// know. The keymap's tables come from [`TableId::config_name`], so the
    /// loader knows every table the core has.
    fn child(self, key: &str) -> Option<Node> {
        match (self, key) {
            (Node::Root, "account") => Some(Node::Account),
            (Node::Root, "ui") => Some(Node::Ui),
            (Node::Root, "files") => Some(Node::Files),
            (Node::Root, "keymap") => Some(Node::Keymap("")),
            (Node::Ui, "room_list") => Some(Node::RoomList),
            (Node::Account, "user_id" | "homeserver")
            | (
                Node::Ui,
                "leader" | "timeoutlen" | "time_format" | "image_preview" | "read_receipts",
            )
            | (Node::RoomList, "visible" | "sort" | "width" | "network")
            | (Node::Files, "download_dir" | "opener") => Some(Node::Setting),
            (Node::Keymap(prefix), key) => TableId::ALL.iter().find_map(|id| {
                let name = id.config_name();
                let rest = match prefix {
                    "" => name,
                    prefix => name.strip_prefix(prefix)?.strip_prefix('.')?,
                };
                match rest.split_once('.') {
                    None => (rest == key).then_some(Node::Bindings),
                    Some((table, _)) => (table == key)
                        .then(|| Node::Keymap(&name[..name.len() - rest.len() + table.len()])),
                }
            }),
            _ => None,
        }
    }
}

/// Removes the keys and tables of `table`, which is `node`, that this
/// version does not know, each with a warning at its key, and goes on into
/// the tables it knows. `setting` is the table's dotted name, empty for the
/// document; it is extended for each key and restored.
fn prune(
    source: &Source<'_>,
    table: &mut DeTable<'_>,
    node: Node,
    setting: &mut String,
    warnings: &mut Vec<ConfigWarning>,
) {
    let mut unknown = Vec::new();
    for (key, value) in table.iter_mut() {
        let name: &str = key.get_ref();
        let len = setting.len();
        if len > 0 {
            setting.push('.');
        }
        setting.push_str(&key_segment(name));
        match node.child(name) {
            None => {
                let reason = if value.get_ref().is_table() {
                    "unknown table"
                } else {
                    "unknown key"
                };
                warnings.push(source.ignored(key, format!("{setting}: {reason}")));
                unknown.push(key.clone());
            }
            Some(Node::Setting | Node::Bindings) => {}
            // A value that is not a table is left to the DTOs' type error.
            Some(child) => {
                if let DeValue::Table(inner) = value.get_mut() {
                    prune(source, inner, child, setting, warnings);
                }
            }
        }
        setting.truncate(len);
    }
    for key in unknown {
        table.remove::<str>(key.get_ref());
    }
}

/// The keys leading to byte `at` of the document: to the innermost value
/// that contains it, or, when `at` is in a key, to the table holding that key.
fn path_in_table<'a>(table: &'a DeTable<'_>, at: usize) -> Option<Vec<&'a str>> {
    table.iter().find_map(|(key, value)| {
        if key.span().contains(&at) {
            return Some(Vec::new());
        }
        let mut path = path_in_value(value, at)?;
        let name: &str = key.get_ref();
        path.insert(0, name);
        Some(path)
    })
}

fn path_in_value<'a>(value: &'a Spanned<DeValue<'_>>, at: usize) -> Option<Vec<&'a str>> {
    // A table under a `[header]` spans only its header, so its keys are
    // searched whether or not the table's span contains `at`.
    let inner = match value.get_ref() {
        DeValue::Table(table) => path_in_table(table, at),
        DeValue::Array(array) => array.iter().find_map(|item| path_in_value(item, at)),
        _ => None,
    };
    inner.or_else(|| value.span().contains(&at).then(Vec::new))
}

fn dotted(path: &[&str]) -> String {
    let segments: Vec<_> = path.iter().map(|key| key_segment(key)).collect();
    segments.join(".")
}

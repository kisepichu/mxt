//! The configuration file (design, "Configuration"): where it is, and how it
//! becomes checked core types. Every mistake is reported with its line and
//! column before the terminal UI starts; none is silently ignored.

mod dto;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use mxt_core::action::{Action, ActionError, Binding};
use mxt_core::ids::UserId;
use mxt_core::keymap::{Keymap, TableId};
use mxt_core::keys::{Key, KeyParseError, KeySeq};
use mxt_core::settings::Settings;
use mxt_tui::TimeFormat;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

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
    /// Absolute.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "file downloads (phase 4) save here")
    )]
    pub download_dir: PathBuf,
    pub opener: Vec<String>,
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
    let dto::File {
        account,
        ui,
        files,
        keymap,
    } = toml::from_str(text).map_err(|error| source.toml_error(&error))?;

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

    let leader = match &ui.leader {
        Some(text) => KeySeq::parse(text.get_ref(), &KeySeq::default())
            .map_err(|error| source.invalid(text, key_error("ui.leader", text, &error)))?,
        None => KeySeq::from_keys(vec![Key::char(' ')]),
    };
    let defaults = Settings::default();
    let settings = Settings {
        timeoutlen_ms: ui.timeoutlen.unwrap_or(defaults.timeoutlen_ms),
        room_sort: ui.room_list.sort.map_or(defaults.room_sort, Into::into),
        room_list_visible: ui.room_list.visible.unwrap_or(defaults.room_list_visible),
        image_preview: ui.image_preview.map_or(defaults.image_preview, Into::into),
        read_receipts: ui.read_receipts.map_or(defaults.read_receipts, Into::into),
    };
    let time_format = match ui.time_format {
        Some(format) => {
            let at = format.span().start;
            TimeFormat::parse(format.into_inner())
                .map_err(|error| source.invalid_at(at, format!("ui.time_format: {error}")))?
        }
        None => TimeFormat::default(),
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

    let bindings = bindings(&source, keymap, &leader)?;
    Ok(Config {
        user_id: UserId::new(user_id.into_inner()),
        homeserver: account.homeserver.map(Spanned::into_inner),
        keymap: Keymap::new(&leader, bindings),
        settings,
        time_format,
        download_dir,
        opener,
    })
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
            } => write!(f, "{}:{line}:{column}: {message}", path.display()),
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

/// The user's bindings of every table. Two spellings of one key sequence in
/// a table (`<leader>x` and `<Space>x`, `<C-a>` and `<C-A>`) are an error at
/// the second, because `Keymap::new` would keep only one of them.
fn bindings(
    source: &Source<'_>,
    keymap: dto::Keymap,
    leader: &KeySeq,
) -> Result<Vec<(TableId, KeySeq, Binding)>, ConfigError> {
    let mut bindings = Vec::new();
    for (id, table) in keymap.into_tables() {
        let setting = format!("keymap.{}", id.config_name());
        let mut entries: Vec<_> = table.into_iter().collect();
        entries.sort_by_key(|(key, _)| key.span().start);
        let mut seen: BTreeMap<KeySeq, Spanned<String>> = BTreeMap::new();
        for (key, value) in entries {
            let keys = KeySeq::parse(key.get_ref(), leader)
                .map_err(|error| source.invalid(&key, key_error(&setting, &key, &error)))?;
            if let Some(first) = seen.get(&keys) {
                return Err(source.invalid(
                    &key,
                    format!(
                        "{setting}: {:?} is the same key sequence as {:?} on line {}",
                        key.get_ref(),
                        first.get_ref(),
                        source.position(first.span().start).0,
                    ),
                ));
            }
            let binding = binding(
                source,
                &format!("{setting}.{}", key_segment(key.get_ref())),
                value,
            )?;
            bindings.push((id, keys.clone(), binding));
            seen.insert(keys, key);
        }
    }
    Ok(bindings)
}

/// Checks the actions of one binding; `setting` names it in messages.
fn binding(
    source: &Source<'_>,
    setting: &str,
    value: Spanned<dto::Binding>,
) -> Result<Binding, ConfigError> {
    let span = value.span();
    let specs = match value.into_inner() {
        dto::Binding::One(spec) => vec![Spanned::new(span.clone(), spec)],
        dto::Binding::Many(specs) => specs,
    };
    let actions = specs
        .into_iter()
        .map(|spec| {
            let spec_ref = spec.get_ref();
            Action::from_spec(&spec_ref.name, &spec_ref.args).map_err(|error| {
                let at = argument_start(spec_ref, &error).unwrap_or(spec.span().start);
                source.invalid_at(at, format!("{setting}: {error}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Binding::new(actions).ok_or_else(|| {
        let message =
            format!("{setting}: an empty array binds nothing; write \"nop\" to remove a binding");
        source.invalid_at(span.start, message)
    })
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
        ActionError::WrongType { arg, .. } | ActionError::InvalidValue { arg, .. } => start(arg),
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

/// The text being loaded, for turning byte offsets into error positions.
struct Source<'a> {
    text: &'a str,
    path: &'a Path,
}

impl Source<'_> {
    fn invalid<T>(&self, at: &Spanned<T>, message: String) -> ConfigError {
        self.invalid_at(at.span().start, message)
    }

    fn invalid_at(&self, at: usize, message: String) -> ConfigError {
        let (line, column) = self.position(at);
        ConfigError::Invalid {
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

    /// A syntax error or a serde error (unknown fields, wrong types, missing
    /// fields), prefixed with the setting it is in when the document parses.
    fn toml_error(&self, error: &toml::de::Error) -> ConfigError {
        let at = error.span().map_or(0, |span| span.start);
        let setting = DeTable::parse(self.text)
            .ok()
            .and_then(|root| path_in_table(root.get_ref(), at).map(|path| dotted(&path)))
            .filter(|setting| !setting.is_empty());
        let message = match setting {
            Some(setting) => format!("{setting}: {}", error.message()),
            None => error.message().to_owned(),
        };
        self.invalid_at(at, message)
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

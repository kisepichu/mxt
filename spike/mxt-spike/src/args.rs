//! Command line parsing with `std::env::args`.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use anyhow::{Context as _, Result, bail};
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId};

pub const USAGE: &str = "\
usage: mxt-spike [--data-dir PATH] <command> [args]

commands:
  login <user_id>
  rooms [--wait SECS] [--json PATH]
  bridge-raw <room_id>
  room <room_id> [--back N] [--wait SECS]
  members <room_id> [--filter TEXT]
  notifications [--limit N]
  classic-probe
  create-test-room
  send <room_id> <text> [--mention USER_ID=TEXT] [--allow-bridged]

The data directory defaults to $XDG_DATA_HOME/mxt-spike (or ~/.local/share/mxt-spike).
Logging goes to stderr; MXT_SPIKE_LOG takes an EnvFilter directive (default: warn).";

pub struct Args {
    pub data_dir: PathBuf,
    pub command: Command,
}

pub enum Command {
    Help,
    Login {
        user_id: OwnedUserId,
    },
    Rooms {
        wait: Duration,
        json: Option<PathBuf>,
    },
    BridgeRaw {
        room_id: OwnedRoomId,
    },
    Room {
        room_id: OwnedRoomId,
        back: usize,
        wait: Duration,
    },
    Members {
        room_id: OwnedRoomId,
        filter: Option<String>,
    },
    Notifications {
        limit: u32,
    },
    ClassicProbe,
    CreateTestRoom,
    Send {
        room_id: OwnedRoomId,
        text: String,
        mention: Option<(OwnedUserId, String)>,
        allow_bridged: bool,
    },
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut args = args.into_iter();
        let mut data_dir = None;
        let command = loop {
            match args.next().as_deref() {
                Some("--data-dir") => {
                    data_dir = Some(PathBuf::from(
                        args.next().context("--data-dir needs a path")?,
                    ));
                }
                Some("-h" | "--help") | None => {
                    return Ok(Self {
                        data_dir: PathBuf::new(),
                        command: Command::Help,
                    });
                }
                Some(command) => break command.to_owned(),
            }
        };
        let data_dir = match data_dir {
            Some(dir) => dir,
            None => default_data_dir()?,
        };
        let rest: Vec<String> = args.collect();

        let command = match command.as_str() {
            "login" => {
                let r = Rest::split(rest, &[], &[])?;
                let [user_id] = r.positional::<1>("login <user_id>")?;
                Command::Login {
                    user_id: user_id.try_into().context("invalid user ID")?,
                }
            }
            "rooms" => {
                let mut r = Rest::split(rest, &["--wait", "--json"], &[])?;
                let wait = r.secs("--wait", 120)?;
                let json = r.take("--json").map(PathBuf::from);
                let [] = r.positional::<0>("rooms")?;
                Command::Rooms { wait, json }
            }
            "bridge-raw" => {
                let r = Rest::split(rest, &[], &[])?;
                let [room_id] = r.positional::<1>("bridge-raw <room_id>")?;
                Command::BridgeRaw {
                    room_id: parse_room_id(room_id)?,
                }
            }
            "room" => {
                let mut r = Rest::split(rest, &["--back", "--wait"], &[])?;
                let back = r.number("--back", 50)?;
                let wait = r.secs("--wait", 30)?;
                let [room_id] = r.positional::<1>("room <room_id>")?;
                Command::Room {
                    room_id: parse_room_id(room_id)?,
                    back,
                    wait,
                }
            }
            "members" => {
                let mut r = Rest::split(rest, &["--filter"], &[])?;
                let filter = r.take("--filter");
                let [room_id] = r.positional::<1>("members <room_id>")?;
                Command::Members {
                    room_id: parse_room_id(room_id)?,
                    filter,
                }
            }
            "notifications" => {
                let mut r = Rest::split(rest, &["--limit"], &[])?;
                let limit = r.number("--limit", 20)?;
                let [] = r.positional::<0>("notifications")?;
                Command::Notifications { limit }
            }
            "classic-probe" => {
                let r = Rest::split(rest, &[], &[])?;
                let [] = r.positional::<0>("classic-probe")?;
                Command::ClassicProbe
            }
            "create-test-room" => {
                let r = Rest::split(rest, &[], &[])?;
                let [] = r.positional::<0>("create-test-room")?;
                Command::CreateTestRoom
            }
            "send" => {
                let mut r = Rest::split(rest, &["--mention"], &["--allow-bridged"])?;
                let allow_bridged = r.flag("--allow-bridged");
                let mention = r.take("--mention").map(|m| parse_mention(&m)).transpose()?;
                let [room_id, text] = r.positional::<2>("send <room_id> <text>")?;
                Command::Send {
                    room_id: parse_room_id(room_id)?,
                    text,
                    mention,
                    allow_bridged,
                }
            }
            other => bail!("unknown command `{other}`"),
        };
        Ok(Self { data_dir, command })
    }
}

fn default_data_dir() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".local/share")
        }
    };
    Ok(base.join("mxt-spike"))
}

fn parse_room_id(room_id: String) -> Result<OwnedRoomId> {
    room_id
        .try_into()
        .context("invalid room ID (expected !id:server)")
}

fn parse_mention(value: &str) -> Result<(OwnedUserId, String)> {
    let (user_id, text) = value
        .split_once('=')
        .context("--mention expects USER_ID=TEXT")?;
    if text.is_empty() {
        bail!("--mention needs a non-empty TEXT");
    }
    Ok((
        user_id.try_into().context("invalid user ID in --mention")?,
        text.to_owned(),
    ))
}

/// The arguments after the command: positional values, `--option VALUE` and `--flag`.
struct Rest {
    positional: Vec<String>,
    values: BTreeMap<String, String>,
    flags: Vec<String>,
}

impl Rest {
    fn split(args: Vec<String>, value_options: &[&str], flags: &[&str]) -> Result<Self> {
        let mut rest = Self {
            positional: Vec::new(),
            values: BTreeMap::new(),
            flags: Vec::new(),
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--" {
                rest.positional.extend(args);
                break;
            } else if value_options.contains(&arg.as_str()) {
                let value = args
                    .next()
                    .with_context(|| format!("{arg} needs a value"))?;
                if rest.values.insert(arg.clone(), value).is_some() {
                    bail!("{arg} given twice");
                }
            } else if flags.contains(&arg.as_str()) {
                rest.flags.push(arg);
            } else if arg.starts_with("--") {
                bail!("unknown option {arg}");
            } else {
                rest.positional.push(arg);
            }
        }
        Ok(rest)
    }

    fn positional<const N: usize>(self, usage: &str) -> Result<[String; N]> {
        self.positional
            .try_into()
            .map_err(|_| anyhow::anyhow!("usage: mxt-spike {usage}"))
    }

    fn take(&mut self, name: &str) -> Option<String> {
        self.values.remove(name)
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }

    fn number<T: std::str::FromStr>(&mut self, name: &str, default: T) -> Result<T> {
        match self.take(name) {
            Some(value) => value
                .parse()
                .map_err(|_| anyhow::anyhow!("{name} expects a number, got `{value}`")),
            None => Ok(default),
        }
    }

    fn secs(&mut self, name: &str, default: u64) -> Result<Duration> {
        self.number(name, default).map(Duration::from_secs)
    }
}

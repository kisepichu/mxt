//! The graphics protocol for images (design, "Files and images"). With
//! `ui.image_preview = "auto"`, mxt asks the terminal for its name itself
//! (XTVERSION) and sets the protocol for the terminals where ratatui-image's
//! own detection goes wrong: over SSH it cannot tell WezTerm (it looks only
//! at `WEZTERM_EXECUTABLE`, which SSH does not forward), so it picks kitty
//! graphics, whose Unicode placeholders WezTerm draws as boxes
//! (wezterm#7924). Inside tmux, images are sixel that tmux stores in the
//! pane, when tmux and its client's terminal support it; else kitty
//! graphics for a kitty or Ghostty client, whose Unicode placeholders are
//! text that tmux places; and else half blocks: tmux passes iTerm2 images,
//! and sixel it does not store, to the outer terminal at the outer cursor,
//! without the pane's offset, and does not track them. Inside WSL without
//! SSH, images are half blocks: every graphics protocol breaks in ConPTY.

use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::os::fd::{AsFd as _, BorrowedFd};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use mxt_core::settings::ImageProtocol;
use ratatui_image::picker::ProtocolType;
use rustix::io::Errno;
use rustix::termios::{self, OptionalActions, SpecialCodeIndex, Termios, Winsize};

/// XTVERSION (`CSI > q`), then DA1 (`CSI c`). Terminals answer in order,
/// and every terminal answers DA1, so its reply ends the wait at once for a
/// terminal that does not know XTVERSION.
const QUERY: &[u8] = b"\x1b[>q\x1b[c";
/// How long [`query_terminal`] waits for the DA1 reply. The answers take one
/// round trip, so this passes only when nothing answers DA1, or on a link
/// slower than that. An answer that comes later goes to ratatui-image's
/// query, which runs next and reads up to its own last answer, so it never
/// reaches the key reader.
const WAIT: Duration = Duration::from_secs(1);
/// How long [`read_name`] waits before it reads a terminal in non-blocking
/// mode again.
const RETRY: Duration = Duration::from_millis(10);
/// The longest DCS string read; tmux keeps 127 bytes of the XTVERSION text.
const MAX_DCS: usize = 256;
/// What [`query_tmux`] asks tmux: its version, whether it was built with
/// sixel (tmux 3.6 and later say so), the window's cell size in pixels, the
/// cell size of the client's terminal in pixels, and the features and the
/// name of the client's terminal. Spaces separate the values, so an empty
/// value, as from a variable an older tmux does not know, keeps its place.
/// The name, which may hold spaces, comes last.
const TMUX_FORMAT: &str = "#{version} #{sixel_support} #{window_cell_width} \
                           #{window_cell_height} #{client_cell_width} \
                           #{client_cell_height} #{client_termfeatures} \
                           #{client_termtype}";
/// How many values [`TMUX_FORMAT`] has.
const TMUX_VALUES: usize = 8;
/// The first tmux version that stores sixel images in the pane.
const TMUX_SIXEL: (u32, u32) = (3, 4);

/// How [`Images::query`](crate::image_view::Images::query) sets up
/// ratatui-image's picker.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Half blocks, without asking the terminal anything.
    Halfblocks,
    /// ratatui-image's query, and the protocol it detects.
    Detected,
    /// ratatui-image's query, and this protocol for the terminal's name if
    /// the query measured the cell size.
    ForTerminal(ProtocolType),
    /// ratatui-image's query for the cell size, and this protocol.
    Forced(ProtocolType),
    /// Sixel that tmux stores in the pane, drawn in tmux's cell size
    /// (width, height) in pixels, without asking the terminal anything.
    TmuxSixel { cell: (u16, u16) },
    /// ratatui-image's kitty graphics inside tmux, in the cell size (width,
    /// height) in pixels of the terminal of tmux's client, without asking
    /// the terminal anything.
    TmuxKitty { cell: (u16, u16) },
}

/// The environment variables the choice depends on. A variable counts as
/// set when it is not empty.
#[derive(Debug)]
pub(crate) struct Env {
    /// `TMUX`: mxt runs in a tmux pane.
    tmux: bool,
    /// `WSL_DISTRO_NAME`: mxt runs in WSL.
    wsl: bool,
    /// `SSH_CONNECTION` or `SSH_TTY`: mxt runs over SSH.
    ssh: bool,
}

impl Env {
    pub(crate) fn read() -> Env {
        Env::from_vars(|name| env::var_os(name))
    }

    fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Env {
        let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
        Env {
            tmux: set("TMUX"),
            wsl: set("WSL_DISTRO_NAME"),
            ssh: set("SSH_CONNECTION") || set("SSH_TTY"),
        }
    }
}

/// The choice for `forced`, the protocol `ui.image_preview` names (`None`
/// for `auto`), in `env`. `tmux` gets tmux's answer, asked only inside tmux
/// with `auto` or `"sixel"`; `name` gets the terminal's name, asked only
/// outside tmux with `auto`. Each is `None` when there is none, unless the
/// environment decides alone.
pub(crate) fn choose(
    forced: Option<ImageProtocol>,
    env: &Env,
    name: impl FnOnce() -> Option<String>,
    tmux: impl FnOnce() -> Option<Tmux>,
) -> Choice {
    match forced {
        // Wherever tmux stores sixel images, even when it does not know
        // whether the terminal draws them: the setting says it does.
        Some(ImageProtocol::Sixel) if env.tmux => {
            return tmux()
                .as_ref()
                .and_then(Tmux::sixel_cell)
                .map_or(Choice::Forced(ProtocolType::Sixel), |cell| {
                    Choice::TmuxSixel { cell }
                });
        }
        Some(protocol) => return Choice::Forced(protocol_type(protocol)),
        None => {}
    }
    // ConPTY stands between WSL and the Windows terminal, unless mxt runs
    // over SSH. Half blocks need no cell size, and ConPTY does not reliably
    // pass the answers to a query on.
    if env.wsl && !env.ssh {
        return Choice::Halfblocks;
    }
    // Inside tmux, sixel that tmux stores, else kitty graphics, whose Unicode
    // placeholders are text that tmux places like any other. tmux passes
    // ratatui-image's iTerm2 images, and sixel it does not store, to the
    // outer terminal at the outer cursor, which is not where the image
    // belongs in the pane, and it neither crops nor clears them; half blocks
    // are text, which tmux handles.
    if env.tmux {
        return tmux().as_ref().map_or(Choice::Halfblocks, Tmux::choice);
    }
    name()
        .as_deref()
        .and_then(protocol_for)
        .map_or(Choice::Detected, Choice::ForTerminal)
}

fn protocol_type(protocol: ImageProtocol) -> ProtocolType {
    match protocol {
        ImageProtocol::Iterm2 => ProtocolType::Iterm2,
        ImageProtocol::Sixel => ProtocolType::Sixel,
        ImageProtocol::Kitty => ProtocolType::Kitty,
        ImageProtocol::Halfblocks => ProtocolType::Halfblocks,
    }
}

/// The protocol for the terminal that `name` names, its XTVERSION text such
/// as `WezTerm 20240203-110809-5046fc22` or `kitty(0.50.0)`. `None` leaves
/// the choice to ratatui-image: for other terminals, and for a tmux in front
/// of SSH, whose answer tells nothing about the terminal outside it. Windows
/// Terminal does not answer XTVERSION; ratatui-image finds sixel in its DA1
/// reply.
fn protocol_for(name: &str) -> Option<ProtocolType> {
    let program = name.split([' ', '(']).next().unwrap_or_default();
    let is = |known: &str| program.eq_ignore_ascii_case(known);
    // ratatui-image's kitty graphics always use Unicode placeholders, which
    // WezTerm draws as boxes.
    if is("WezTerm") || is("iTerm2") {
        Some(ProtocolType::Iterm2)
    } else if is("ghostty") || is("kitty") {
        Some(ProtocolType::Kitty)
    } else {
        None
    }
}

/// Asks the terminal for its name and version with [`QUERY`], and reads the
/// answers up to the end of the DA1 reply and no further: whatever follows
/// stays in the terminal's input for the next reader. The terminal must be
/// in raw mode, and nothing else may read its input. `None` when no
/// XTVERSION reply came before the DA1 reply or within [`WAIT`], and when
/// the terminal cannot be asked or read: the name only refines the choice,
/// so it never keeps mxt from starting.
pub(crate) fn query_terminal() -> Option<String> {
    ask_name(io::stdin().as_fd(), &mut io::stdout(), WAIT)
}

/// [`query_terminal`] with `tty`, the terminal's input, `out`, its output,
/// and `wait` in place of [`WAIT`].
fn ask_name(tty: BorrowedFd<'_>, out: &mut impl Write, wait: Duration) -> Option<String> {
    // Not a terminal (`mxt </dev/null`, where crossterm uses `/dev/tty`):
    // ask nothing, so that no answer arrives in the shell after mxt.
    let raw = termios::tcgetattr(tty).ok()?;
    out.write_all(QUERY).and_then(|()| out.flush()).ok()?;
    let mut input = Tty {
        fd: tty,
        termios: raw.clone(),
        tenths: None,
        deadline: Instant::now() + wait,
    };
    let name = read_name(&mut input);
    // ratatui-image's query, next, waits in a blocking read. If even this
    // fails, mxt still starts, as after any other failure here.
    let _ = termios::tcsetattr(tty, OptionalActions::Now, &raw);
    name
}

/// tmux's answer to [`TMUX_FORMAT`].
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Tmux {
    /// `#{version}` as (major, minor), `None` unless it is a release such as
    /// `3.6a`, or a development version such as `next-3.7`.
    version: Option<(u32, u32)>,
    /// `#{sixel_support}` is `1`: tmux was built with sixel.
    sixel_support: bool,
    /// `#{window_cell_width}` and `#{window_cell_height}`, nonzero: the cell
    /// size in pixels with which tmux turns an image's pixels into cells.
    cell: Option<(u16, u16)>,
    /// `#{client_cell_width}` and `#{client_cell_height}`, nonzero: the cell
    /// size in pixels of the terminal of the client tmux takes when several
    /// are attached.
    client_cell: Option<(u16, u16)>,
    /// `#{client_termfeatures}` has `sixel`: the terminal of that client
    /// draws sixel.
    client_sixel: bool,
    /// `#{client_termtype}`: that terminal's answer to the XTVERSION query
    /// that tmux sends it, such as `kitty(0.50.0)`; empty when tmux does not
    /// know it.
    client_name: String,
}

impl Tmux {
    /// `answer`, [`TMUX_FORMAT`] expanded by tmux. A value that cannot be
    /// read counts as missing.
    fn parse(answer: &str) -> Tmux {
        let mut values = answer.trim_end_matches('\n').splitn(TMUX_VALUES, ' ');
        let mut next = || values.next().unwrap_or_default();
        let size = |width: &str, height: &str| -> Option<(u16, u16)> {
            let width = width.parse().ok().filter(|&width| width > 0);
            let height = height.parse().ok().filter(|&height| height > 0);
            width.zip(height)
        };
        let version = tmux_version(next());
        let sixel_support = next() == "1";
        let cell = size(next(), next());
        let client_cell = size(next(), next());
        let client_sixel = next().split(',').any(|feature| feature == "sixel");
        Tmux {
            version,
            sixel_support,
            cell,
            client_cell,
            client_sixel,
            client_name: next().to_owned(),
        }
    }

    /// The choice with `auto`: sixel that tmux stores where tmux draws it
    /// for its client; else ratatui-image's kitty graphics where the
    /// client's terminal is kitty or Ghostty, by the names mxt maps outside
    /// tmux, and tmux knows its cell size; and else half blocks.
    fn choice(&self) -> Choice {
        if self.client_draws_sixel()
            && let Some(cell) = self.sixel_cell()
        {
            return Choice::TmuxSixel { cell };
        }
        let kitty = protocol_for(&self.client_name) == Some(ProtocolType::Kitty);
        match self.client_cell {
            Some(cell) if kitty => Choice::TmuxKitty { cell },
            _ => Choice::Halfblocks,
        }
    }

    /// Whether tmux draws sixel images for its client: tmux shows a client
    /// a placeholder in their place unless its terminal has the `sixel`
    /// feature and tmux knows its cell size in pixels
    /// (`tty_cmd_sixelimage` in tmux's tty.c).
    fn client_draws_sixel(&self) -> bool {
        self.client_sixel && self.client_cell.is_some()
    }

    /// tmux's cell size when tmux stores sixel images in the pane: from
    /// version 3.4, if built with sixel, which only tmux 3.6 and later
    /// report.
    fn sixel_cell(&self) -> Option<(u16, u16)> {
        let stores = self.version.is_some_and(|version| version >= TMUX_SIXEL);
        self.cell.filter(|_| stores && self.sixel_support)
    }
}

/// A tmux version, `3.6a` or `next-3.7`, as (major, minor).
fn tmux_version(text: &str) -> Option<(u32, u32)> {
    let text = text.strip_prefix("next-").unwrap_or(text);
    let (major, rest) = text.split_once('.')?;
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    Some((major.parse().ok()?, rest[..digits].parse().ok()?))
}

/// Asks tmux with [`TMUX_FORMAT`] about the pane mxt runs in and the client
/// tmux takes for it. `None` when tmux cannot be asked: the answer only
/// refines the choice, so it never keeps mxt from starting.
pub(crate) fn query_tmux() -> Option<Tmux> {
    let output = Command::new("tmux")
        .args(["display", "-p", TMUX_FORMAT])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| Tmux::parse(&String::from_utf8_lossy(&output.stdout)))
}

/// tmux's cell size in pixels, read again after mxt's pane resized: the
/// pane's size in pixels over its size in cells, which tmux sets to its
/// cell size times the pane's columns and rows whenever it resizes the pane
/// (`window_pane_send_resize` in tmux's window.c), so no tmux runs. When the
/// pane has no size in pixels, tmux's `#{window_cell_*}`. `None` when
/// neither is known.
pub(crate) fn tmux_cell() -> Option<(u16, u16)> {
    termios::tcgetwinsize(io::stdout())
        .ok()
        .and_then(|size| window_cell(&size))
        .or_else(|| query_tmux().and_then(|tmux| tmux.cell))
}

/// The cell size in pixels from `size`, a terminal's size in cells and in
/// pixels, as ratatui-image computes it, when the terminal reports both
/// and a cell is at least a pixel.
pub(crate) fn window_cell(size: &Winsize) -> Option<(u16, u16)> {
    let cell = (
        size.ws_xpixel.checked_div(size.ws_col)?,
        size.ws_ypixel.checked_div(size.ws_row)?,
    );
    (cell.0 > 0 && cell.1 > 0).then_some(cell)
}

/// The terminal's input, one byte at a time, until a deadline.
trait Input {
    /// The next byte, or `None` once the deadline has passed without one.
    fn next_byte(&mut self) -> rustix::io::Result<Option<u8>>;
}

/// The XTVERSION text from `input`, read up to the end of the DA1 reply, or
/// as far as it came by the deadline. `None` also when a read fails.
fn read_name(input: &mut impl Input) -> Option<String> {
    let mut replies = Replies::default();
    loop {
        match input.next_byte() {
            Ok(Some(byte)) => {
                if replies.push(byte) {
                    break;
                }
            }
            Ok(None) => break,
            // A signal came.
            Err(Errno::INTR) => {}
            // The terminal is in non-blocking mode (`O_NONBLOCK`, which a
            // program before mxt can leave on), where a read returns at
            // once instead of after `VTIME`. Read on until the deadline, so
            // that this reader still takes the answers up to the DA1 reply.
            Err(Errno::AGAIN) => thread::sleep(RETRY),
            Err(_) => return None,
        }
    }
    replies.name
}

/// The terminal's input in raw mode, read with `VMIN` 0 and `VTIME` up to
/// the deadline: a read returns after `VTIME` without input, so no thread is
/// left waiting in a blocking read. This works on every terminal device,
/// while macOS's `poll` does not support devices. One byte per read leaves
/// the bytes after the DA1 reply unread, and bypasses std's buffer of stdin.
struct Tty<'a> {
    fd: BorrowedFd<'a>,
    /// The raw mode's settings, with `VMIN` and `VTIME` changed once read.
    termios: Termios,
    /// The `VTIME` set last, in tenths of a second.
    tenths: Option<u8>,
    deadline: Instant,
}

impl Input for Tty<'_> {
    fn next_byte(&mut self) -> rustix::io::Result<Option<u8>> {
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            let tenths = u8::try_from(left.as_millis().div_ceil(100))
                .unwrap_or(u8::MAX)
                .max(1);
            if self.tenths != Some(tenths) {
                self.termios.special_codes[SpecialCodeIndex::VMIN] = 0;
                self.termios.special_codes[SpecialCodeIndex::VTIME] = tenths;
                termios::tcsetattr(self.fd, OptionalActions::Now, &self.termios)?;
                self.tenths = Some(tenths);
            }
            let mut byte = [0];
            // Nothing once `VTIME` passed without input: wait on until the
            // deadline.
            if rustix::io::read(self.fd, &mut byte)? == 1 {
                return Ok(Some(byte[0]));
            }
        }
    }
}

/// The answers to [`QUERY`], byte by byte: the XTVERSION reply
/// `DCS > | text ST` and the DA1 reply `CSI ? ... c`, which ends them. Any
/// other input, such as keys typed before the answers, is skipped.
#[derive(Default)]
struct Replies {
    state: State,
    /// The DCS string being read, `None` once it is longer than [`MAX_DCS`].
    dcs: Option<Vec<u8>>,
    /// The XTVERSION text.
    name: Option<String>,
}

#[derive(Clone, Copy, Default)]
enum State {
    #[default]
    Ground,
    /// After ESC.
    Escape,
    /// In a DCS string.
    Dcs,
    /// After ESC in a DCS string, which `\` ends (ST).
    DcsEscape,
    /// In a control sequence: `start` before its first byte, `da1` once
    /// that is `?`, as in the DA1 reply.
    Csi { start: bool, da1: bool },
}

impl Replies {
    /// Takes the next byte; `true` once it ends the DA1 reply.
    fn push(&mut self, byte: u8) -> bool {
        const ESC: u8 = 0x1b;
        self.state = match (self.state, byte) {
            (State::Dcs, ESC) => State::DcsEscape,
            (State::Dcs, _) => {
                if let Some(dcs) = &mut self.dcs {
                    if dcs.len() < MAX_DCS {
                        dcs.push(byte);
                    } else {
                        self.dcs = None;
                    }
                }
                State::Dcs
            }
            (State::DcsEscape, b'\\') => {
                self.end_dcs();
                State::Ground
            }
            // An ESC that cut the DCS string short starts a new sequence.
            (State::DcsEscape, _) => {
                self.state = State::Escape;
                return self.push(byte);
            }
            (_, ESC) => State::Escape,
            (State::Escape, b'P') => {
                self.dcs = Some(Vec::new());
                State::Dcs
            }
            (State::Escape, b'[') => State::Csi {
                start: true,
                da1: false,
            },
            // Parameter and intermediate bytes.
            (State::Csi { start, da1 }, 0x20..=0x3f) => State::Csi {
                start: false,
                da1: da1 || (start && byte == b'?'),
            },
            (State::Csi { da1: true, .. }, b'c') => {
                self.state = State::Ground;
                return true;
            }
            // Another sequence's final byte, or a byte outside sequences.
            (State::Ground | State::Escape | State::Csi { .. }, _) => State::Ground,
        };
        false
    }

    fn end_dcs(&mut self) {
        let dcs = self.dcs.take();
        if let Some(text) = dcs.as_deref().and_then(|dcs| dcs.strip_prefix(b">|")) {
            let text = String::from_utf8_lossy(text);
            let text = text.trim();
            if !text.is_empty() {
                self.name = Some(text.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs::File;

    use rustix::fs::{Mode, OFlags};
    use rustix::pty::OpenptFlags;

    use super::*;

    // Real answers. The XTVERSION texts are the `xtversion_raw` values and
    // the DA1 replies the `device_attributes.raw` values in
    // jquast/ucs-detect's `data/<terminal>.yaml` (commit 6547497), except
    // WezTerm's text, which is the owner's (the format of
    // `term/src/terminalstate/mod.rs`: `DCS >|{program} {version} ST`). The
    // sources agree: kitty's `screen_xtversion` (kitty/screen.c) writes
    // `>|kitty(<version>)`, Ghostty's `reportXtversion`
    // (src/termio/stream_handler.zig) `ESC P>|ghostty <version> ESC \`, and
    // tmux's input.c `ESC P>|tmux <version> ESC \`.
    const WEZTERM: &[u8] = b"\x1bP>|WezTerm 20240203-110809-5046fc22\x1b\\\x1b[?65;4;6;18;22;52c";
    const KITTY: &[u8] = b"\x1bP>|kitty(0.50.0)\x1b\\\x1b[?62;52;c";
    const GHOSTTY: &[u8] = b"\x1bP>|ghostty 1.4.1-main+2fc8d02c2\x1b\\\x1b[?62;22;52c";
    const ITERM2: &[u8] = b"\x1bP>|iTerm2 3.7.7\x1b\\\x1b[?64;1;2;4;6;17;18;21;22;52c";
    const TMUX: &[u8] = b"\x1bP>|tmux 3.4\x1b\\\x1b[?1;2;4c";
    /// Windows Terminal 1.24 answers only DA1: it does not implement
    /// XTVERSION (microsoft/terminal#18382, closed as not planned).
    const WINDOWS_TERMINAL: &[u8] = b"\x1b[?61;4;6;7;14;21;22;23;24;28;32;42;52c";

    /// Input that has `items`, bytes or errors, and then nothing until the
    /// deadline.
    struct Script(VecDeque<rustix::io::Result<u8>>);

    impl Script {
        fn new(bytes: &[u8]) -> Script {
            Script(bytes.iter().copied().map(Ok).collect())
        }

        /// The bytes not read; no error may be left.
        fn unread(&self) -> Vec<u8> {
            self.0.iter().map(|item| item.unwrap()).collect()
        }
    }

    impl Input for Script {
        fn next_byte(&mut self) -> rustix::io::Result<Option<u8>> {
            self.0.pop_front().transpose()
        }
    }

    fn name_in(bytes: &[u8]) -> Option<String> {
        read_name(&mut Script::new(bytes))
    }

    #[test]
    fn the_name_is_the_xtversion_text() {
        for (answers, name) in [
            (WEZTERM, "WezTerm 20240203-110809-5046fc22"),
            (KITTY, "kitty(0.50.0)"),
            (GHOSTTY, "ghostty 1.4.1-main+2fc8d02c2"),
            (ITERM2, "iTerm2 3.7.7"),
            (TMUX, "tmux 3.4"),
        ] {
            assert_eq!(name_in(answers).as_deref(), Some(name));
        }
    }

    #[test]
    fn the_da1_reply_alone_ends_the_answers_without_a_name() {
        let mut input = Script::new(&[WINDOWS_TERMINAL, b"j".as_slice()].concat());
        assert_eq!(read_name(&mut input), None);
        assert_eq!(input.unread(), b"j");
    }

    #[test]
    fn reading_stops_at_the_end_of_the_da1_reply() {
        // Keys typed after the answers, and an answer of another query.
        let after: &[u8] = b"jk\x1b[A\x1b[0n";
        let mut input = Script::new(&[WEZTERM, after].concat());
        assert_eq!(
            read_name(&mut input).as_deref(),
            Some("WezTerm 20240203-110809-5046fc22")
        );
        assert_eq!(input.unread(), after);
    }

    #[test]
    fn reading_goes_on_after_a_signal_and_in_non_blocking_mode() {
        // A read of a terminal in non-blocking mode fails with `EAGAIN`
        // until input comes, here also between pieces of the answers.
        let (first, rest) = WEZTERM.split_at(10);
        let mut items = VecDeque::from([Err(Errno::AGAIN), Err(Errno::INTR), Err(Errno::AGAIN)]);
        items.extend(first.iter().copied().map(Ok));
        items.push_back(Err(Errno::AGAIN));
        items.extend(rest.iter().copied().map(Ok));
        items.push_back(Ok(b'j'));
        let mut input = Script(items);
        assert_eq!(
            read_name(&mut input).as_deref(),
            Some("WezTerm 20240203-110809-5046fc22")
        );
        assert_eq!(input.unread(), b"j");
    }

    #[test]
    fn a_failed_read_ends_the_answers_without_a_name() {
        // The read after the XTVERSION reply fails.
        let da1 = KITTY.iter().rposition(|&byte| byte == 0x1b).unwrap();
        let mut input = Script::new(KITTY);
        input.0.insert(da1, Err(Errno::IO));
        assert_eq!(read_name(&mut input), None);
        assert_eq!(input.unread(), &KITTY[da1..]);
    }

    #[test]
    fn input_that_is_no_terminal_is_not_asked() {
        let null = File::open("/dev/null").unwrap();
        let mut out = Vec::new();
        assert_eq!(
            ask_name(null.as_fd(), &mut out, Duration::from_secs(5)),
            None
        );
        assert!(out.is_empty());
    }

    #[test]
    fn a_reply_split_across_reads_is_put_together() {
        // Cut inside the DCS introducer, the ST and the DA1 reply.
        let pieces: [&[u8]; 5] = [
            b"\x1bP",
            b">|kitty(0.5",
            b"0.0)\x1b",
            b"\\\x1b[?62",
            b";52;c",
        ];
        assert_eq!(pieces.concat(), KITTY);
        let mut replies = Replies::default();
        let (last, first) = pieces.split_last().unwrap();
        for piece in first {
            assert!(!piece.iter().any(|&byte| replies.push(byte)));
        }
        let ends: Vec<bool> = last.iter().map(|&byte| replies.push(byte)).collect();
        assert_eq!(ends, [false, false, false, false, true]);
        assert_eq!(replies.name.as_deref(), Some("kitty(0.50.0)"));
    }

    #[test]
    fn without_the_da1_reply_the_name_so_far_counts_at_the_deadline() {
        let xtversion = &TMUX[..TMUX.iter().position(|&byte| byte == b'\\').unwrap() + 1];
        assert_eq!(name_in(xtversion).as_deref(), Some("tmux 3.4"));
        // Cut before the ST.
        assert_eq!(name_in(&xtversion[..xtversion.len() - 2]), None);
        assert_eq!(name_in(b""), None);
    }

    #[test]
    fn garbage_around_the_answers_is_skipped() {
        let mut long = b"\x1bP>|".to_vec();
        long.extend([b'x'; 300]);
        long.extend(b"\x1b\\");
        let garbage: &[&[u8]] = &[
            // Keys typed before the answers: text, an arrow key, a lone
            // `<Esc>`, `<A-x>` and a focus event.
            b"ab\x1b[A\x1b\x1bx\x1b[I",
            // A DECRQSS answer, a DCS string without text, and an
            // over-long one.
            b"\x1bP1$r0m\x1b\\\x1bP>|\x1b\\",
            &long,
            // A DA2 reply, which is not the end.
            b"\x1b[>1;10;0c",
            // A DCS string that an ESC cuts short.
            b"\x1bP>|cut",
        ];
        let mut answers = garbage.concat();
        answers.extend_from_slice(GHOSTTY);
        assert_eq!(
            name_in(&answers).as_deref(),
            Some("ghostty 1.4.1-main+2fc8d02c2")
        );
        assert_eq!(name_in(&garbage.concat()), None);
        // Invalid UTF-8 in the text.
        assert_eq!(
            name_in(b"\x1bP>|Wez\xffTerm\x1b\\\x1b[?62c").as_deref(),
            Some("Wez\u{fffd}Term")
        );
    }

    #[test]
    fn terminals_get_protocols_by_name() {
        for (name, protocol) in [
            (
                "WezTerm 20240203-110809-5046fc22",
                Some(ProtocolType::Iterm2),
            ),
            ("iTerm2 3.7.7", Some(ProtocolType::Iterm2)),
            ("ghostty 1.4.1-main+2fc8d02c2", Some(ProtocolType::Kitty)),
            ("kitty(0.50.0)", Some(ProtocolType::Kitty)),
            // Others, from ucs-detect's `xterm.yaml`, `foot.yaml` and
            // `gnometerminal.yaml` (VTE): ratatui-image decides.
            ("tmux 3.4", None),
            ("XTerm(411)", None),
            ("foot(1.28.0)", None),
            ("VTE(8400)", None),
            ("WezTermX 1", None),
            ("", None),
        ] {
            assert_eq!(protocol_for(name), protocol, "{name}");
        }
    }

    fn env(vars: &[(&str, &str)]) -> Env {
        Env::from_vars(|name| {
            vars.iter()
                .find(|(set, _)| *set == name)
                .map(|(_, value)| value.into())
        })
    }

    const IN_TMUX: (&str, &str) = ("TMUX", "/tmp/tmux-1000/default,1234,0");
    /// `#{client_termfeatures}` for a terminal that draws sixel, in tmux
    /// 3.6a's order (`tty_features` in tty-features.c).
    const WITH_SIXEL: &str =
        "256,bpaste,clipboard,cstyle,extkeys,focus,margins,mouse,rectfill,RGB,sixel,sync,title";
    /// The same terminal without sixel.
    const WITHOUT_SIXEL: &str =
        "256,bpaste,clipboard,cstyle,extkeys,focus,margins,mouse,rectfill,RGB,sync,title";
    /// Terminal names as tmux keeps them in `#{client_termtype}` (the
    /// XTVERSION texts above).
    const WEZTERM_NAME: &str = "WezTerm 20240203-110809-5046fc22";
    const GHOSTTY_NAME: &str = "ghostty 1.4.1-main+2fc8d02c2";

    /// What [`choose`] asked for.
    #[derive(Debug, Default, PartialEq)]
    struct Asked {
        name: bool,
        tmux: bool,
    }

    /// The choice for `forced` in `env` when the terminal's name is `name`
    /// and tmux answers `tmux`, and what it asked for.
    fn choice(
        forced: Option<ImageProtocol>,
        env: &Env,
        name: Option<&str>,
        tmux: Option<&str>,
    ) -> (Choice, Asked) {
        let mut asked = Asked::default();
        let choice = choose(
            forced,
            env,
            || {
                asked.name = true;
                name.map(str::to_owned)
            },
            || {
                asked.tmux = true;
                tmux.map(Tmux::parse)
            },
        );
        (choice, asked)
    }

    const NAME: Asked = Asked {
        name: true,
        tmux: false,
    };
    const TMUX_ANSWER: Asked = Asked {
        name: false,
        tmux: true,
    };

    #[test]
    fn auto_outside_tmux_asks_the_terminal_and_maps_its_name() {
        let ssh = env(&[("SSH_CONNECTION", "192.0.2.1 50000 192.0.2.2 22")]);
        let wezterm = Some("WezTerm 20240203-110809-5046fc22");
        assert_eq!(
            choice(None, &ssh, wezterm, None),
            (Choice::ForTerminal(ProtocolType::Iterm2), NAME)
        );
        // A local tmux in front of SSH answers for the terminal.
        assert_eq!(
            choice(None, &ssh, Some("tmux 3.4"), None),
            (Choice::Detected, NAME)
        );
        assert_eq!(
            choice(None, &env(&[]), None, None),
            (Choice::Detected, NAME)
        );
        assert_eq!(
            choice(None, &env(&[("TMUX", "")]), wezterm, None),
            (Choice::ForTerminal(ProtocolType::Iterm2), NAME)
        );
    }

    #[test]
    fn auto_inside_tmux_uses_the_sixel_tmux_stores_else_kitty_or_half_blocks() {
        let tmux = env(&[IN_TMUX]);
        let stored = &Choice::TmuxSixel { cell: (9, 18) };
        // In the cell size of the client's terminal, which draws it.
        let kitty = &Choice::TmuxKitty { cell: (10, 20) };
        let half = &Choice::Halfblocks;
        for (answer, expected) in [
            (format!("3.6a 1 9 18 10 20 {WITH_SIXEL}\n"), stored),
            (format!("3.4 1 9 18 10 20 {WITH_SIXEL}"), stored),
            (format!("next-3.7 1 9 18 10 20 {WITH_SIXEL}"), stored),
            // Sixel that tmux stores wins over the terminal's name.
            (
                format!("3.6a 1 9 18 10 20 {WITH_SIXEL} kitty(0.50.0)"),
                stored,
            ),
            (
                format!("3.6a 1 9 18 10 20 {WITH_SIXEL} {WEZTERM_NAME}"),
                stored,
            ),
            // Otherwise kitty and Ghostty get kitty graphics, whichever
            // tmux, as long as tmux knows their cell size.
            (
                format!("3.6a 1 9 18 10 20 {WITHOUT_SIXEL} kitty(0.50.0)"),
                kitty,
            ),
            (
                format!("3.6a 1 9 18 10 20 {WITHOUT_SIXEL} {GHOSTTY_NAME}"),
                kitty,
            ),
            (
                format!("3.6a 0 9 18 10 20 {WITH_SIXEL} kitty(0.50.0)"),
                kitty,
            ),
            (
                format!("3.3a  9 18 10 20 {WITHOUT_SIXEL} kitty(0.50.0)"),
                kitty,
            ),
            (
                format!("3.6a 1 9 18 0 0 {WITHOUT_SIXEL} kitty(0.50.0)"),
                half,
            ),
            // Other terminals without sixel, or without a name, get half
            // blocks, WezTerm and iTerm2 too.
            (
                format!("3.6a 1 9 18 10 20 {WITHOUT_SIXEL} {WEZTERM_NAME}"),
                half,
            ),
            (
                format!("3.6a 1 9 18 10 20 {WITHOUT_SIXEL} iTerm2 3.7.7"),
                half,
            ),
            (format!("3.6a 1 9 18 10 20 {WITHOUT_SIXEL}"), half),
            // No client is attached.
            ("3.6a 1 9 18     \n".to_owned(), half),
            // tmux does not know the size of the terminal's cells in
            // pixels, so it would show a placeholder.
            (format!("3.6a 1 9 18 0 0 {WITH_SIXEL}"), half),
            (format!("3.6a 1 9 18 10 0 {WITH_SIXEL}"), half),
            // tmux was built without sixel, or does not say (3.5a).
            (format!("3.6a 0 9 18 10 20 {WITH_SIXEL}"), half),
            (format!("3.5a  9 18 10 20 {WITH_SIXEL}"), half),
            // Too old, or a version that cannot be read.
            (format!("3.3a 1 9 18 10 20 {WITH_SIXEL}"), half),
            (format!("openbsd-7.4 1 9 18 10 20 {WITH_SIXEL}"), half),
        ] {
            let (got, asked) = choice(None, &tmux, Some(WEZTERM_NAME), Some(&answer));
            assert_eq!((&got, asked), (expected, TMUX_ANSWER), "{answer}");
        }
        // tmux cannot be asked.
        assert_eq!(
            choice(None, &tmux, None, None),
            (Choice::Halfblocks, TMUX_ANSWER)
        );
    }

    #[test]
    fn forced_sixel_inside_tmux_goes_to_tmux_where_tmux_stores_it() {
        let tmux = env(&[IN_TMUX]);
        // Whatever the client's features and cell size say: the setting
        // says the terminal draws sixel.
        let without = format!("3.6a 1 9 18 0 0 {WITHOUT_SIXEL}");
        assert_eq!(
            choice(Some(ImageProtocol::Sixel), &tmux, None, Some(&without)),
            (Choice::TmuxSixel { cell: (9, 18) }, TMUX_ANSWER)
        );
        // Otherwise ratatui-image wraps it for passthrough.
        for answer in [
            Some("3.6a 0 9 18 10 20 sixel"),
            Some("3.3a  9 18 10 20 sixel"),
            None,
        ] {
            assert_eq!(
                choice(Some(ImageProtocol::Sixel), &tmux, None, answer),
                (Choice::Forced(ProtocolType::Sixel), TMUX_ANSWER),
                "{answer:?}"
            );
        }
        // The other protocols always go through ratatui-image.
        for (forced, protocol) in [
            (ImageProtocol::Iterm2, ProtocolType::Iterm2),
            (ImageProtocol::Kitty, ProtocolType::Kitty),
            (ImageProtocol::Halfblocks, ProtocolType::Halfblocks),
        ] {
            assert_eq!(
                choice(Some(forced), &tmux, None, Some(&without)),
                (Choice::Forced(protocol), Asked::default())
            );
        }
    }

    #[test]
    fn tmux_answers_are_read_by_position() {
        // The terminal's name, last, keeps its spaces.
        assert_eq!(
            Tmux::parse(&format!("3.6a 1 9 18 10 20 {WITH_SIXEL} {GHOSTTY_NAME}\n")),
            Tmux {
                version: Some((3, 6)),
                sixel_support: true,
                cell: Some((9, 18)),
                client_cell: Some((10, 20)),
                client_sixel: true,
                client_name: GHOSTTY_NAME.to_owned(),
            }
        );
        // tmux 3.3a does not know `sixel_support`.
        assert_eq!(
            Tmux::parse(&format!("3.3a  9 18 10 20 {WITH_SIXEL}\n")),
            Tmux {
                version: Some((3, 3)),
                cell: Some((9, 18)),
                client_cell: Some((10, 20)),
                client_sixel: true,
                ..Tmux::default()
            }
        );
        // Cell sizes of zero, a feature that only contains `sixel`, and an
        // answer cut short.
        assert_eq!(
            Tmux::parse("3.4 1 0 18 10 0 nosixel,RGB"),
            Tmux {
                version: Some((3, 4)),
                sixel_support: true,
                ..Tmux::default()
            }
        );
        assert_eq!(Tmux::parse("3.6a 1"), Tmux::parse("3.6a 1   "));
        assert_eq!(Tmux::parse(""), Tmux::default());
        assert_eq!(tmux_version("next-3.7"), Some((3, 7)));
        assert_eq!(tmux_version("3.4-rc"), Some((3, 4)));
        for version in ["openbsd-7.4", "master", "3", "3.", ".4", "3.x"] {
            assert_eq!(tmux_version(version), None, "{version}");
        }
    }

    #[test]
    fn the_cell_size_is_the_size_in_pixels_over_the_size_in_cells() {
        let cell = |(ws_col, ws_row, ws_xpixel, ws_ypixel)| {
            window_cell(&Winsize {
                ws_row,
                ws_col,
                ws_xpixel,
                ws_ypixel,
            })
        };
        // What tmux sets for a pane of 80x23 cells while its window's cell
        // size is 10x20, and of 100x29 cells at 16x32.
        assert_eq!(cell((80, 23, 800, 460)), Some((10, 20)));
        assert_eq!(cell((100, 29, 1600, 928)), Some((16, 32)));
        // No size in pixels, or in cells, or less than a pixel per cell.
        for size in [
            (80, 23, 0, 0),
            (80, 23, 800, 0),
            (0, 0, 800, 460),
            (80, 23, 40, 460),
        ] {
            assert_eq!(cell(size), None, "{size:?}");
        }
    }

    #[test]
    fn wsl_without_ssh_gets_half_blocks_without_asking() {
        let wsl = ("WSL_DISTRO_NAME", "Ubuntu");
        for vars in [&[wsl][..], &[wsl, IN_TMUX][..], &[wsl, ("SSH_TTY", "")][..]] {
            assert_eq!(
                choice(None, &env(vars), Some("kitty(0.50.0)"), None),
                (Choice::Halfblocks, Asked::default())
            );
        }
        for ssh in [
            ("SSH_CONNECTION", "192.0.2.1 50000 192.0.2.2 22"),
            ("SSH_TTY", "/dev/pts/3"),
        ] {
            assert_eq!(
                choice(None, &env(&[wsl, ssh]), Some("kitty(0.50.0)"), None),
                (Choice::ForTerminal(ProtocolType::Kitty), NAME)
            );
        }
        assert_eq!(
            choice(None, &env(&[("WSL_DISTRO_NAME", "")]), None, None),
            (Choice::Detected, NAME)
        );
    }

    #[test]
    fn a_forced_protocol_outside_tmux_asks_nothing_and_holds_in_wsl() {
        let wsl = env(&[("WSL_DISTRO_NAME", "Ubuntu")]);
        let elsewhere = env(&[]);
        for (forced, protocol) in [
            (ImageProtocol::Iterm2, ProtocolType::Iterm2),
            (ImageProtocol::Sixel, ProtocolType::Sixel),
            (ImageProtocol::Kitty, ProtocolType::Kitty),
            (ImageProtocol::Halfblocks, ProtocolType::Halfblocks),
        ] {
            for env in [&wsl, &elsewhere] {
                assert_eq!(
                    choice(Some(forced), env, None, None),
                    (Choice::Forced(protocol), Asked::default())
                );
            }
        }
    }

    /// A pseudo-terminal: its master end, and its slave end in raw mode as
    /// crossterm sets it up (`VMIN` 1, `VTIME` 0). `None`, with a message,
    /// where no pseudo-terminal can be opened.
    fn open_pty() -> Option<(File, File)> {
        let opened = (|| -> rustix::io::Result<(File, File)> {
            let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
            rustix::pty::grantpt(&master)?;
            rustix::pty::unlockpt(&master)?;
            let name = rustix::pty::ptsname(&master, Vec::new())?;
            let slave = rustix::fs::open(
                name.as_c_str(),
                OFlags::RDWR | OFlags::NOCTTY,
                Mode::empty(),
            )?;
            Ok((File::from(master), File::from(slave)))
        })();
        let (master, slave) = match opened {
            Ok(pty) => pty,
            Err(error) => {
                eprintln!("skipped: no pseudo-terminal can be opened here: {error}");
                return None;
            }
        };
        let mut raw = termios::tcgetattr(&slave).unwrap();
        raw.make_raw();
        raw.special_codes[SpecialCodeIndex::VMIN] = 1;
        raw.special_codes[SpecialCodeIndex::VTIME] = 0;
        termios::tcsetattr(&slave, OptionalActions::Now, &raw).unwrap();
        Some((master, slave))
    }

    /// All of `tty`'s settings: `Termios` has no `PartialEq`, and its `Debug`
    /// shows every field.
    fn settings(tty: &File) -> String {
        format!("{:?}", termios::tcgetattr(tty).unwrap())
    }

    /// Up to `len` bytes that `tty` has to read, waiting up to a second for
    /// each read.
    fn read_up_to(tty: &File, len: usize) -> Vec<u8> {
        let mut timed = termios::tcgetattr(tty).unwrap();
        timed.special_codes[SpecialCodeIndex::VMIN] = 0;
        timed.special_codes[SpecialCodeIndex::VTIME] = 10;
        termios::tcsetattr(tty, OptionalActions::Now, &timed).unwrap();
        let mut bytes = vec![0; len];
        let mut filled = 0;
        while filled < len {
            match rustix::io::read(tty, &mut bytes[filled..]).unwrap() {
                0 => break,
                read => filled += read,
            }
        }
        bytes.truncate(filled);
        bytes
    }

    #[test]
    fn on_a_terminal_reading_stops_after_the_da1_reply() {
        let Some((master, slave)) = open_pty() else {
            return;
        };
        let before = settings(&slave);
        let after: &[u8] = b"jk\x1b[0n";
        let mut out = Vec::new();
        let name = thread::scope(|scope| {
            // The answers come in two pieces while the reader waits, and
            // keys and another answer follow them at once.
            scope.spawn(|| {
                let (first, rest) = WEZTERM.split_at(10);
                thread::sleep(Duration::from_millis(50));
                (&master).write_all(first).unwrap();
                thread::sleep(Duration::from_millis(50));
                (&master).write_all(&[rest, after].concat()).unwrap();
            });
            ask_name(slave.as_fd(), &mut out, Duration::from_secs(5))
        });
        assert_eq!(out, QUERY);
        assert_eq!(name.as_deref(), Some("WezTerm 20240203-110809-5046fc22"));
        // `VMIN` and `VTIME` too.
        assert_eq!(settings(&slave), before);
        assert_eq!(read_up_to(&slave, after.len()), after);
    }

    #[test]
    fn on_a_terminal_that_does_not_answer_reading_ends_at_the_deadline() {
        let Some((_master, slave)) = open_pty() else {
            return;
        };
        let before = settings(&slave);
        let wait = Duration::from_millis(200);
        let started = Instant::now();
        assert_eq!(ask_name(slave.as_fd(), &mut io::sink(), wait), None);
        let took = started.elapsed();
        assert!(
            took >= wait && took < wait + Duration::from_secs(2),
            "took {took:?}"
        );
        assert_eq!(settings(&slave), before);
    }

    #[test]
    fn on_a_terminal_in_non_blocking_mode_the_answers_are_still_read() {
        let Some((master, slave)) = open_pty() else {
            return;
        };
        // Reads fail with `EAGAIN` until the answers come.
        rustix::io::ioctl_fionbio(&slave, true).unwrap();
        let name = thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(50));
                (&master)
                    .write_all(&[KITTY, b"j".as_slice()].concat())
                    .unwrap();
            });
            ask_name(slave.as_fd(), &mut io::sink(), Duration::from_secs(5))
        });
        assert_eq!(name.as_deref(), Some("kitty(0.50.0)"));
        rustix::io::ioctl_fionbio(&slave, false).unwrap();
        assert_eq!(read_up_to(&slave, 1), b"j");
    }
}

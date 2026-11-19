//! The graphics protocol for images (design, "Files and images"). With
//! `ui.image_preview = "auto"`, mxt asks the terminal for its name itself
//! (XTVERSION), or tmux for the outer terminal's name, and sets the protocol
//! for the terminals where ratatui-image's own detection goes wrong: over
//! SSH it cannot tell WezTerm (it looks only at `WEZTERM_EXECUTABLE`, which
//! SSH does not forward), so it picks kitty graphics, whose Unicode
//! placeholders WezTerm draws as boxes (wezterm#7924). Inside WSL without
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
use rustix::termios::{self, OptionalActions, SpecialCodeIndex, Termios};

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
}

/// Where the terminal's name comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameSource {
    /// The terminal's answer to XTVERSION, from [`query_terminal`].
    Terminal,
    /// tmux, from [`tmux_client_termtype`]: inside tmux, tmux answers
    /// XTVERSION itself.
    Tmux,
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
/// for `auto`), in `env`. With `auto`, `name` gets the terminal's name from
/// the source, `None` when it has none, unless the environment decides
/// alone.
pub(crate) fn choose(
    forced: Option<ImageProtocol>,
    env: &Env,
    name: impl FnOnce(NameSource) -> Option<String>,
) -> Choice {
    if let Some(protocol) = forced {
        return Choice::Forced(protocol_type(protocol));
    }
    // ConPTY stands between WSL and the Windows terminal, unless mxt runs
    // over SSH. Half blocks need no cell size, and ConPTY does not reliably
    // pass the answers to a query on.
    if env.wsl && !env.ssh {
        return Choice::Halfblocks;
    }
    let source = if env.tmux {
        NameSource::Tmux
    } else {
        NameSource::Terminal
    };
    name(source)
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
/// the choice to ratatui-image: for other terminals, and for tmux, whose
/// answer tells nothing about the terminal outside it. Windows Terminal
/// does not answer XTVERSION; ratatui-image finds sixel in its DA1 reply.
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

/// tmux's `client_termtype`: the outer terminal's answer to the XTVERSION
/// query that tmux (3.2 and later) sends it, for the client tmux takes when
/// several are attached. `None` when tmux does not know it.
pub(crate) fn tmux_client_termtype() -> Option<String> {
    let output = Command::new("tmux")
        .args(["display", "-p", "#{client_termtype}"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout);
    let name = name.trim();
    (output.status.success() && !name.is_empty()).then(|| name.to_owned())
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

    /// The choice with `auto` in `env` when the terminal's name is `name`,
    /// and where the name came from.
    fn auto(env: &Env, name: Option<&str>) -> (Choice, Option<NameSource>) {
        let mut asked = None;
        let choice = choose(None, env, |source| {
            asked = Some(source);
            name.map(str::to_owned)
        });
        (choice, asked)
    }

    #[test]
    fn auto_asks_the_terminal_and_maps_its_name() {
        let ssh = env(&[("SSH_CONNECTION", "192.0.2.1 50000 192.0.2.2 22")]);
        assert_eq!(
            auto(&ssh, Some("WezTerm 20240203-110809-5046fc22")),
            (
                Choice::ForTerminal(ProtocolType::Iterm2),
                Some(NameSource::Terminal)
            )
        );
        // A local tmux in front of SSH answers for the terminal.
        assert_eq!(
            auto(&ssh, Some("tmux 3.4")),
            (Choice::Detected, Some(NameSource::Terminal))
        );
        assert_eq!(
            auto(&env(&[]), None),
            (Choice::Detected, Some(NameSource::Terminal))
        );
    }

    #[test]
    fn inside_tmux_the_name_comes_from_tmux() {
        let tmux = env(&[("TMUX", "/tmp/tmux-1000/default,1234,0")]);
        assert_eq!(
            auto(&tmux, Some("WezTerm 20240203-110809-5046fc22")),
            (
                Choice::ForTerminal(ProtocolType::Iterm2),
                Some(NameSource::Tmux)
            )
        );
        assert_eq!(
            auto(&tmux, Some("ghostty 1.4.1-main+2fc8d02c2")),
            (
                Choice::ForTerminal(ProtocolType::Kitty),
                Some(NameSource::Tmux)
            )
        );
        // An outer terminal that did not answer tmux.
        assert_eq!(
            auto(&tmux, None),
            (Choice::Detected, Some(NameSource::Tmux))
        );
        assert_eq!(
            auto(&env(&[("TMUX", "")]), None),
            (Choice::Detected, Some(NameSource::Terminal))
        );
    }

    #[test]
    fn wsl_without_ssh_gets_half_blocks_without_asking() {
        let wsl = ("WSL_DISTRO_NAME", "Ubuntu");
        let tmux = ("TMUX", "/tmp/tmux-1000/default,1234,0");
        for vars in [&[wsl][..], &[wsl, tmux][..], &[wsl, ("SSH_TTY", "")][..]] {
            assert_eq!(
                auto(&env(vars), Some("kitty(0.50.0)")),
                (Choice::Halfblocks, None)
            );
        }
        for ssh in [
            ("SSH_CONNECTION", "192.0.2.1 50000 192.0.2.2 22"),
            ("SSH_TTY", "/dev/pts/3"),
        ] {
            assert_eq!(
                auto(&env(&[wsl, ssh]), Some("kitty(0.50.0)")),
                (
                    Choice::ForTerminal(ProtocolType::Kitty),
                    Some(NameSource::Terminal)
                )
            );
        }
        assert_eq!(
            auto(&env(&[("WSL_DISTRO_NAME", "")]), None),
            (Choice::Detected, Some(NameSource::Terminal))
        );
    }

    #[test]
    fn a_forced_protocol_needs_no_name_and_holds_in_wsl() {
        let wsl = env(&[("WSL_DISTRO_NAME", "Ubuntu")]);
        let elsewhere = env(&[]);
        for (forced, protocol) in [
            (ImageProtocol::Iterm2, ProtocolType::Iterm2),
            (ImageProtocol::Sixel, ProtocolType::Sixel),
            (ImageProtocol::Kitty, ProtocolType::Kitty),
            (ImageProtocol::Halfblocks, ProtocolType::Halfblocks),
        ] {
            for env in [&wsl, &elsewhere] {
                let choice = choose(Some(forced), env, |_| panic!("asked for the name"));
                assert_eq!(choice, Choice::Forced(protocol));
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

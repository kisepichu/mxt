//! `files.opener` (design, "Files and images"): the command that opens
//! downloaded files and links. Its argument comes from a message, so it is
//! passed as one argument and never through a shell.

use std::io;
use std::process::{Command, Stdio};
use std::thread;

pub(crate) struct Opener {
    /// The program and its arguments, from `files.opener`.
    command: Vec<String>,
}

impl Opener {
    pub(crate) fn new(command: Vec<String>) -> Opener {
        Opener { command }
    }

    /// Runs the opener with `target` appended, detached: stdin, stdout and
    /// stderr are null, and it gets its own process group, so neither its
    /// output nor the terminal's signals reach it. A thread reaps it when it
    /// exits; mxt never waits for it.
    pub(crate) fn open(&self, target: &str) -> io::Result<()> {
        let mut child = self
            .command(target)
            .ok_or_else(|| io::Error::other("files.opener is empty"))?
            .spawn()?;
        // Without the thread, the exited opener stays a zombie until mxt
        // exits, which is all a failure to start it costs.
        let _ = thread::Builder::new()
            .name("opener".to_owned())
            .spawn(move || child.wait());
        Ok(())
    }

    /// The opener's program and arguments, then `target` as one more
    /// argument.
    fn command(&self, target: &str) -> Option<Command> {
        let (program, args) = self.command.split_first()?;
        let mut command = Command::new(program);
        command
            .args(args)
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        Some(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_is_one_argument_after_the_opener_and_its_arguments() {
        let opener = Opener::new(vec!["wslview".into(), "--new-window".into()]);
        let target = "https://example.org/a b;$(touch x)'\"&|";
        let command = opener.command(target).unwrap();
        assert_eq!(command.get_program(), "wslview");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--new-window", target]
        );
    }

    #[test]
    fn an_opener_that_cannot_start_is_an_error() {
        let missing = Opener::new(vec!["/nonexistent/mxt-opener".into()]);
        assert!(missing.open("/tmp/cat.png").is_err());
        assert!(Opener::new(Vec::new()).open("/tmp/cat.png").is_err());
    }
}

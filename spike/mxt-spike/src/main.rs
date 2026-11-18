//! Throwaway phase 0 spike: answers the design's questions about matrix-sdk
//! 0.19 against a real account. It lives only on the `spike/matrix` branch and
//! is never merged (see `docs/superpowers/plans/2026-11-19-phase0-spike.md`).
//!
//! Safety rules, enforced here: `send` only goes to the recorded test room
//! unless `--allow-bridged` is given; no receipts, read markers, unread marks,
//! notification settings, joins, leaves or invites; `login` refuses to run
//! while a session exists; no secrets in the output.

// matrix-sdk's futures are deeply nested; its documentation asks for this.
#![recursion_limit = "256"]

mod args;
mod bridge;
mod classic;
mod login;
mod members;
mod notifications;
mod room;
mod rooms;
mod session;
mod test_room;
mod util;

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use tracing_subscriber::EnvFilter;

use crate::{
    args::{Args, Command, USAGE},
    session::DataDir,
};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_env("MXT_SPIKE_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("error: {error:#}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = tokio::select! {
        result = run(args) => result,
        _ = tokio::signal::ctrl_c() => Err(anyhow!("interrupted")),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<ExitCode> {
    let dir = DataDir::new(args.data_dir);
    match args.command {
        Command::Help => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Login { user_id } => login::run(&dir, &user_id).await,
        Command::Rooms { wait, json } => rooms::run(&dir, wait, json.as_deref())
            .await
            .map(|()| ExitCode::SUCCESS),
        Command::BridgeRaw { room_id } => bridge::run_raw(&dir, &room_id)
            .await
            .map(|()| ExitCode::SUCCESS),
        Command::Room {
            room_id,
            back,
            wait,
        } => room::run(&dir, &room_id, back, wait)
            .await
            .map(|()| ExitCode::SUCCESS),
        Command::Members { room_id, filter } => members::run(&dir, &room_id, filter.as_deref())
            .await
            .map(|()| ExitCode::SUCCESS),
        Command::Notifications { limit } => notifications::run(&dir, limit)
            .await
            .map(|()| ExitCode::SUCCESS),
        Command::ClassicProbe => classic::run(&dir).await.map(|()| ExitCode::SUCCESS),
        Command::CreateTestRoom => test_room::create(&dir).await.map(|()| ExitCode::SUCCESS),
        Command::Send {
            room_id,
            text,
            mention,
            allow_bridged,
        } => test_room::send(
            &dir,
            test_room::SendArgs {
                room_id: &room_id,
                text: &text,
                mention: mention.as_ref(),
                allow_bridged,
            },
        )
        .await
        .map(|()| ExitCode::SUCCESS),
    }
}

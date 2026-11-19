//! The event loop: terminal input, timers and the matrix adapter's messages
//! into `App::update`, its effects out to the adapters, and a frame after
//! every batch.

use std::collections::VecDeque;
use std::io;
use std::time::Instant;

use mxt_core::app::App;
use mxt_core::effect::{Effect, MatrixEffect};
use mxt_core::msg::Msg;
use mxt_tui::Tui;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::warn;

/// Runs until `UiEffect::Quit`. `from_matrix` delivers adapter messages;
/// matrix effects are sent to `to_matrix`.
pub async fn run(
    app: &mut App,
    tui: &mut Tui,
    mut from_matrix: UnboundedReceiver<Msg>,
    to_matrix: UnboundedSender<MatrixEffect>,
) -> io::Result<()> {
    let mut queue = VecDeque::new();
    draw(app, tui, &to_matrix, &mut queue)?;
    while !tui.should_quit() {
        let deadline = tui.next_deadline();
        let timer = async move {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = tui.next_event() => match event {
                Some(event) => queue.extend(tui.handle_event(app, event?)),
                // The terminal input has ended, so nothing can quit later.
                None => return Ok(()),
            },
            Some(msg) = from_matrix.recv() => queue.push_back(msg),
            () = timer => queue.extend(tui.expired(Instant::now())),
        }
        // A burst of room updates draws once.
        while let Ok(msg) = from_matrix.try_recv() {
            queue.push_back(msg);
        }
        update(app, tui, &to_matrix, &mut queue);
        if !tui.should_quit() {
            draw(app, tui, &to_matrix, &mut queue)?;
        }
    }
    Ok(())
}

/// Draws, then feeds the viewport messages of the frame to `update`.
fn draw(
    app: &mut App,
    tui: &mut Tui,
    to_matrix: &UnboundedSender<MatrixEffect>,
    queue: &mut VecDeque<Msg>,
) -> io::Result<()> {
    queue.extend(tui.draw(app)?);
    update(app, tui, to_matrix, queue);
    Ok(())
}

/// Feeds the queued messages to `update` in order and routes the effects.
/// What the tui adapter reports while executing an effect (the composer's
/// text and edits) joins the end of the queue. Every failure for the status
/// line passes here, so this is where it is logged.
fn update(
    app: &mut App,
    tui: &mut Tui,
    to_matrix: &UnboundedSender<MatrixEffect>,
    queue: &mut VecDeque<Msg>,
) {
    while let Some(msg) = queue.pop_front() {
        log_failure(&msg);
        for effect in app.update(msg) {
            match effect {
                Effect::Ui(effect) => queue.extend(tui.execute(effect, Instant::now())),
                Effect::Matrix(effect) => {
                    if let Err(SendError(effect)) = to_matrix.send(effect) {
                        let error = "The matrix adapter has stopped".to_owned();
                        // Every send is answered, so the core lets go of its
                        // draft.
                        queue.push_back(match effect {
                            MatrixEffect::Send { id, .. } => Msg::SendFailed { id, error },
                            _ => Msg::EffectFailed { error },
                        });
                    }
                }
            }
        }
    }
}

/// Logs a failure of an effect at `warn` with the status line's text, which
/// the adapters keep brief: no room IDs and no message contents (design,
/// "Error handling"). The matrix adapter logs only the full error, at
/// `debug`, so nothing is logged twice.
fn log_failure(msg: &Msg) {
    if let Msg::EffectFailed { error } | Msg::SendFailed { error, .. } = msg {
        warn!("{error}");
    }
}

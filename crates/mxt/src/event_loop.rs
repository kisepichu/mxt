//! The event loop: terminal input, timers and the matrix adapter's messages
//! into `App::update`, its effects out to the adapters, and a frame after
//! every batch.

use std::collections::VecDeque;
use std::io;
use std::time::Instant;

use crossterm::event::EventStream;
use futures_util::StreamExt;
use mxt_core::app::App;
use mxt_core::effect::{Effect, MatrixEffect};
use mxt_core::msg::Msg;
use mxt_tui::Tui;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Runs until `UiEffect::Quit`. `from_matrix` delivers adapter messages;
/// matrix effects are sent to `to_matrix`.
pub async fn run(
    app: &mut App,
    tui: &mut Tui,
    mut from_matrix: UnboundedReceiver<Msg>,
    to_matrix: UnboundedSender<MatrixEffect>,
) -> io::Result<()> {
    let mut terminal_events = EventStream::new();
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
            event = terminal_events.next() => match event {
                Some(event) => queue.extend(tui.handle_event(event?)),
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
fn update(
    app: &mut App,
    tui: &mut Tui,
    to_matrix: &UnboundedSender<MatrixEffect>,
    queue: &mut VecDeque<Msg>,
) {
    while let Some(msg) = queue.pop_front() {
        for effect in app.update(msg) {
            match effect {
                Effect::Ui(effect) => tui.execute(effect, Instant::now()),
                Effect::Matrix(effect) => {
                    if to_matrix.send(effect).is_err() {
                        queue.push_back(Msg::EffectFailed {
                            error: "The matrix adapter has stopped".to_owned(),
                        });
                    }
                }
            }
        }
    }
}

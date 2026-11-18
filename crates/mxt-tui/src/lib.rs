//! The terminal adapter of mxt: layout, widgets, input routing and UI
//! effects.

mod input;
mod terminal;
mod timers;
mod view;

pub use input::key_from_crossterm;
pub use terminal::{Tui, install_panic_hook};

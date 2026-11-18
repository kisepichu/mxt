//! The terminal adapter of mxt: layout, widgets, input routing and UI
//! effects.

mod clipboard;
mod input;
mod message_view;
mod terminal;
mod timeline_view;
mod timers;
mod view;

pub use input::key_from_crossterm;
pub use terminal::{Tui, install_panic_hook};
pub use timeline_view::check_time_format;

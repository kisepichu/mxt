//! Domain and application logic of mxt.
//!
//! This crate performs no I/O and reads no clock. It depends only on `std`
//! and small pure crates, so its tests build without matrix-rust-sdk.

pub mod action;
pub mod app;
pub mod command;
pub mod effect;
pub mod emoji;
pub mod fuzzy;
pub mod history;
pub mod ids;
pub mod keymap;
pub mod keys;
pub mod line;
pub mod mention;
pub mod msg;
pub mod resolver;
pub mod rich;
pub mod room;
pub mod rooms;
pub mod search;
pub mod settings;
pub mod timeline;

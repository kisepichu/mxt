//! Domain and application logic of mxt.
//!
//! This crate performs no I/O and reads no clock. It depends only on `std`
//! and small pure crates, so its tests build without matrix-rust-sdk.

pub mod fuzzy;
pub mod ids;
pub mod room;
pub mod rooms;

//! Zellij pinentry plugin — shared modules.
//!
//! The plugin entry point is in `main.rs` (wasm binary target).
//! This library exposes backend, protocol, and UI modules for use by
//! both the plugin binary and native-target tests.

pub mod backend;
pub mod protocol;
pub mod ui;

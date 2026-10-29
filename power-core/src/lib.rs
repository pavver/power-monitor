//! Platform-independent logic of the mains power monitor. Everything here runs
//! on the host under `cargo test`; the `firmware` crate only wires it to ESP-IDF.

pub mod power;
pub mod message;
pub mod form;
pub mod config;

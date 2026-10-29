//! Platform-independent logic of the mains power monitor. Everything here runs
//! on the host under `cargo test`; the `firmware` crate only wires it to ESP-IDF.

pub mod power;
pub mod message;
pub mod form;
pub mod config;
pub mod dns;
pub mod outbox;
pub mod telegram;
pub mod html;
pub mod template;

//! Platform-independent logic of the mains power monitor. Everything here runs
//! on the host under `cargo test`; the `firmware` crate only wires it to ESP-IDF.

pub mod power;
pub mod message;
pub mod form;
pub mod json;
pub mod dns;
pub mod outbox;
pub mod telegram;
pub mod settings;
pub mod api;
pub mod ota;
pub mod auth;
pub mod reset;
pub mod portal;
pub mod radio;
pub mod jk;
pub mod adc;
pub mod remote;
pub mod clock;
pub mod texts;
#[cfg(test)]
mod jk_frames;

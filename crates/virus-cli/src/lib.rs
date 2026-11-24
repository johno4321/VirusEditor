//! `virus`: a headless command-line tool for live sessions with the Access Virus Rack.
//!
//! Message building ([`messages`]), answer collection ([`transport`]), probe planning
//! ([`probe`]), monitor decoding ([`decode`]) and the self-test ([`selftest`]) are pure and
//! unit-tested against an in-memory fake Virus; [`midi_link`] is the thin real-MIDI layer.

#![forbid(unsafe_code)]

pub mod address;
pub mod cli;
pub mod commands;
pub mod decode;
pub mod files;
pub mod messages;
pub mod midi_link;
pub mod ops;
pub mod probe;
pub mod selftest;
pub mod settings;
pub mod stamp;
pub mod summary;
pub mod transport;

#[cfg(test)]
mod fake;
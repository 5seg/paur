//! paur-cli: command-line front-end for paur.
//!
//! Talks to a running paur daemon over its HTTP API (over TCP, since
//! axum 0.7 doesn't accept a unix listener). Read commands can also
//! be served by reading the DB directly when the daemon is offline —
//! see [`cmd`] for the per-command implementation.

pub mod client;
pub mod cmd;
pub mod output;

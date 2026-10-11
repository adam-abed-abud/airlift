//! Airlift: temporary, PIN-protected file sharing over the local network.
//!
//! The crate is a library plus a thin binary so integration tests can build
//! the router and drive it in-process without opening a socket.

pub mod auth;
pub mod cli;
pub mod files;
pub mod server;
pub mod transfer;
pub mod utils;

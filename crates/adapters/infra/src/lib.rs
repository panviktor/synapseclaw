//! Shared infrastructure adapters used by multiple higher-level crates
//! (channels, gateway, tools, onboard).
//!
//! Contains config I/O, identity management, and approval flow.

pub mod approval;
pub mod config_io;
pub mod identity;

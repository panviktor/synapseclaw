//! Port definitions for the fork-owned application core.
//!
//! Ports define capabilities the core needs from the outside world.
//! Adapters (in `fork_adapters`) implement these ports.

pub mod channel_registry;
pub mod conversation_store;

//! Application services — fork-owned business logic.
//!
//! Each service owns a domain concern and orchestrates through ports.
//! Services are the *only* place where business policy lives;
//! adapters translate, infrastructure executes.

pub mod delivery_service;
pub mod inbound_message_service;

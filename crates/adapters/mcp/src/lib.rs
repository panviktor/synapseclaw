//! MCP (Model Context Protocol) client stack.
//!
//! Provides JSON-RPC transport, client registry, deferred tool stubs,
//! and tool wrappers for MCP server integration.

pub mod mcp_protocol;
pub mod mcp_transport;
pub mod mcp_client;
pub mod mcp_tool;
pub mod mcp_deferred;
pub mod tool_search;

// Re-export primary types for ergonomic imports.
pub use mcp_client::McpRegistry;
pub use mcp_deferred::{ActivatedToolSet, DeferredMcpToolSet};
pub use mcp_tool::McpToolWrapper;

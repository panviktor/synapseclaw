//! Middleware adapters — concrete implementations of ToolMiddlewarePort.
//!
//! Phase 4.1 Slice 3:
//! - `RateLimitMiddleware` — per-tool call rate limiting
//! - `ValidationMiddleware` — JSON Schema validation on tool args
//! - `ApprovalGateMiddleware` — human-in-the-loop approval

pub mod approval_gate;
pub mod rate_limit;
pub mod validation;

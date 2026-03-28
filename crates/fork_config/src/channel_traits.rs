//! Channel configuration traits — minimal interface for channel config types.

/// The trait for describing a channel config.
pub trait ChannelConfig {
    /// Human-readable name.
    fn name() -> &'static str;
    /// Short description.
    fn desc() -> &'static str;
}

/// Object-safe variant for dynamic dispatch.
pub trait ConfigHandle {
    fn name(&self) -> &'static str;
    fn desc(&self) -> &'static str;
}

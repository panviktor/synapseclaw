//! Channel presentation policy — keeps human-facing channel UX separate from
//! execution telemetry.
//!
//! Channels are conversational surfaces, not log streams. Full tool traces live
//! in the web/operator UI. Channel adapters should only expose detailed tool
//! events when the operator explicitly opts into verbose mode.

use crate::domain::channel::ChannelCapability;
use std::time::Duration;

/// How much execution detail may be rendered into human messaging channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelPresentationMode {
    /// Default human-first mode: no raw tool trace, at most compact milestones.
    Compact,
    /// Explicit opt-in debug mode: raw tool trace may be shown by the adapter.
    Verbose,
}

impl ChannelPresentationMode {
    /// Backward-compatible mapping from the legacy `show_tool_calls` flag.
    pub fn from_show_tool_calls(show_tool_calls: bool) -> Self {
        if show_tool_calls {
            Self::Verbose
        } else {
            Self::Compact
        }
    }
}

/// Compact progress surface for long-running turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactProgressSurface {
    None,
    StatusMessage,
}

const COMPACT_PROGRESS_DELAY_SECS: u64 = 6;
const COMPACT_PROGRESS_TEXT: &str = "Working on it...";

/// Whether raw tool trace should be rendered into the channel.
pub fn tool_trace_enabled(mode: ChannelPresentationMode) -> bool {
    matches!(mode, ChannelPresentationMode::Verbose)
}

/// Decide whether the channel should receive a compact progress status.
///
/// Rules:
/// - verbose mode does not need a synthetic compact status
/// - channels with streaming drafts already have a progressive surface
/// - channels with typing indicators already have a lightweight activity signal
/// - otherwise, emit a single delayed status message for long turns
pub fn compact_progress_surface(
    mode: ChannelPresentationMode,
    caps: &[ChannelCapability],
    supports_streaming: bool,
) -> CompactProgressSurface {
    if tool_trace_enabled(mode)
        || supports_streaming
        || caps.contains(&ChannelCapability::Typing)
    {
        CompactProgressSurface::None
    } else {
        CompactProgressSurface::StatusMessage
    }
}

/// Delay before compact progress becomes user-visible.
pub fn compact_progress_delay() -> Duration {
    Duration::from_secs(COMPACT_PROGRESS_DELAY_SECS)
}

/// Stable copy for a compact long-running status update.
pub fn compact_progress_text() -> &'static str {
    COMPACT_PROGRESS_TEXT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_flag_maps_to_mode() {
        assert_eq!(
            ChannelPresentationMode::from_show_tool_calls(false),
            ChannelPresentationMode::Compact
        );
        assert_eq!(
            ChannelPresentationMode::from_show_tool_calls(true),
            ChannelPresentationMode::Verbose
        );
    }

    #[test]
    fn compact_progress_disabled_for_streaming_and_typing() {
        assert_eq!(
            compact_progress_surface(ChannelPresentationMode::Compact, &[], true),
            CompactProgressSurface::None
        );
        assert_eq!(
            compact_progress_surface(
                ChannelPresentationMode::Compact,
                &[ChannelCapability::Typing],
                false
            ),
            CompactProgressSurface::None
        );
    }

    #[test]
    fn compact_progress_enabled_for_plain_text_channels() {
        assert_eq!(
            compact_progress_surface(ChannelPresentationMode::Compact, &[], false),
            CompactProgressSurface::StatusMessage
        );
    }

    #[test]
    fn verbose_mode_disables_compact_progress() {
        assert_eq!(
            compact_progress_surface(ChannelPresentationMode::Verbose, &[], false),
            CompactProgressSurface::None
        );
        assert!(tool_trace_enabled(ChannelPresentationMode::Verbose));
    }
}

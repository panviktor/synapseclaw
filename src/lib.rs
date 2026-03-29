#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::assigning_clones,
    clippy::bool_to_int_with_if,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::cast_possible_wrap,
    clippy::doc_markdown,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::implicit_clone,
    clippy::items_after_statements,
    clippy::map_unwrap_or,
    clippy::manual_let_else,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::new_without_default,
    clippy::needless_pass_by_value,
    clippy::needless_raw_string_hashes,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unnecessary_cast,
    clippy::unnecessary_lazy_evaluations,
    clippy::unnecessary_literal_bound,
    clippy::unnecessary_map_or,
    clippy::unused_self,
    clippy::cast_precision_loss,
    clippy::unnecessary_wraps,
    dead_code
)]

pub mod agent;
pub use crate::adapters::channels;
pub(crate) mod adapters;
pub mod config;
pub use crate::adapters::gateway;
/// Re-export fork_core workspace crate so `crate::synapse_core::` paths keep working.
pub use synapse_core;
pub(crate) mod identity;
pub mod memory;
pub(crate) mod multimodal;
pub use crate::adapters::providers;
pub mod runtime;
pub(crate) mod security;
pub(crate) mod skills;
/// Re-export hooks for integration tests.
pub use crate::adapters::hooks;
/// Re-export observability for tests/benches.
pub use crate::adapters::observability;
pub use crate::adapters::tools;
pub(crate) mod util;

pub use config::Config;

// CLI command enums — canonical definitions in fork_config.
pub use synapse_config::commands::{
    ChannelCommands, CronCommands, GatewayCommands, IntegrationCommands, MemoryCommands,
    ServiceCommands, SkillCommands,
};

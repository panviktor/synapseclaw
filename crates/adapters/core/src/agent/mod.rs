#[allow(clippy::module_inception)]
pub mod agent;
mod autosave;
pub mod classifier;
pub mod context_engine;
pub mod dispatcher;
pub mod prompt;
pub mod run_context;
pub mod runner_adapter;
mod runtime_loop;
mod tool_repair_classification;
pub mod turn_context_fmt;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use agent::{Agent, AgentBuilder, AgentRuntimePorts};
pub(crate) use autosave::autosave_memory_key;
pub(crate) use runtime_loop::{
    execute_one_tool, run_tool_call_loop, ToolExecutionOutcome, ToolLoopRouteCapabilities,
};
#[allow(unused_imports)]
pub use runtime_loop::{process_message, resolve_agent_id, run, run_with_shared_memory};

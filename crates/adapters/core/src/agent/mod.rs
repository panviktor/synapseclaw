#[allow(clippy::module_inception)]
pub mod agent;
pub mod classifier;
pub mod context_engine;
pub mod dispatcher;
pub mod loop_;
pub mod prompt;
pub mod run_context;
pub mod runner_adapter;
pub mod turn_context_fmt;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use agent::{Agent, AgentBuilder};
#[allow(unused_imports)]
pub use loop_::{process_message, run, run_with_shared_memory};

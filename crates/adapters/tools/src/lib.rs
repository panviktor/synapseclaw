#![allow(dead_code)]
//! Tool implementations for the SynapseClaw agent runtime.
//!
//! Each tool implements `synapse_domain::ports::tool::Tool`.
//! Tools with agent/gateway dependencies (delegate, node_tool, agents_ipc)
//! remain in `synapse_adapters` core crate.

pub mod backup_tool;
pub mod browser;
#[cfg(feature = "browser-native")]
pub mod browser_delegate;
pub mod browser_open;
pub mod clarify;
pub mod cli_discovery;
pub mod cloud_ops;
pub mod cloud_patterns;
pub mod composio;
pub mod content_search;
pub mod core_memory_update;
pub mod cron_add;
pub mod cron_list;
pub mod cron_remove;
pub mod cron_run;
pub mod cron_runs;
pub mod cron_update;
pub mod data_management;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod git_operations;
pub mod glob_search;
pub mod google_workspace;
pub mod http_request;
pub mod image_info;
pub mod knowledge_tool;
pub mod linkedin;
pub mod linkedin_client;
pub mod memory_forget;
pub mod memory_recall;
pub mod memory_store;
pub mod message_send;
pub mod microsoft365;
pub mod model_routing_config;
pub mod notion_tool;
#[cfg(feature = "rag-pdf")]
pub mod pdf_read;
pub mod precedent_search;
pub mod project_intel;
pub mod proxy_config;
pub mod pushover;
pub mod report_templates;
pub mod schedule;
pub mod schema;
pub mod screenshot;
pub mod security_ops;
pub mod session_search;
pub mod shell;
pub mod standing_order;
pub mod swarm;
pub mod tavily_extract;
pub mod telegram_post;
pub mod todo;
pub mod traits;
pub mod web_fetch;
pub mod web_search_tool;
pub mod workspace_tool;

// Re-export key types
pub use synapse_domain::ports::tool::ArcToolRef;
pub use traits::{Tool, ToolResult, ToolSpec};

pub use backup_tool::BackupTool;
pub use browser::{BrowserTool, ComputerUseConfig};
#[cfg(feature = "browser-native")]
pub use browser_delegate::{BrowserDelegateConfig, BrowserDelegateTool};
pub use browser_open::BrowserOpenTool;
pub use cloud_ops::CloudOpsTool;
pub use cloud_patterns::CloudPatternsTool;
pub use composio::ComposioTool;
pub use content_search::ContentSearchTool;
pub use cron_add::CronAddTool;
pub use cron_list::CronListTool;
pub use cron_remove::CronRemoveTool;
pub use cron_run::CronRunTool;
pub use cron_runs::CronRunsTool;
pub use cron_update::CronUpdateTool;
pub use data_management::DataManagementTool;
pub use file_edit::FileEditTool;
pub use file_read::FileReadTool;
pub use file_write::FileWriteTool;
pub use git_operations::GitOperationsTool;
pub use glob_search::GlobSearchTool;
pub use google_workspace::GoogleWorkspaceTool;
pub use http_request::HttpRequestTool;
pub use image_info::ImageInfoTool;
pub use knowledge_tool::KnowledgeTool;
pub use linkedin::LinkedInTool;
pub use memory_forget::MemoryForgetTool;
pub use memory_recall::MemoryRecallTool;
pub use memory_store::MemoryStoreTool;
pub use microsoft365::Microsoft365Tool;
pub use model_routing_config::ModelRoutingConfigTool;
pub use notion_tool::NotionTool;
pub use precedent_search::PrecedentSearchTool;
pub use project_intel::ProjectIntelTool;
pub use proxy_config::ProxyConfigTool;
pub use pushover::PushoverTool;
pub use report_templates::ReportTemplate;
pub use schedule::ScheduleTool;
pub use screenshot::ScreenshotTool;
pub use security_ops::SecurityOpsTool;
pub use shell::ShellTool;
pub use swarm::SwarmTool;
pub use web_fetch::WebFetchTool;
pub use web_search_tool::WebSearchTool;
pub use workspace_tool::WorkspaceTool;

//! Factory: build SecurityPolicy from config values.

use std::path::Path;
use synapse_domain::config::schema::AutonomyConfig;
use synapse_domain::domain::security_policy::{expand_user_path, ActionTracker, SecurityPolicy};

/// Build SecurityPolicy from config sections.
pub fn security_policy_from_config(
    autonomy_config: &AutonomyConfig,
    workspace_dir: &Path,
) -> SecurityPolicy {
    SecurityPolicy {
        autonomy: autonomy_config.level,
        workspace_dir: workspace_dir.to_path_buf(),
        workspace_only: autonomy_config.workspace_only,
        allowed_commands: autonomy_config.allowed_commands.clone(),
        forbidden_paths: autonomy_config.forbidden_paths.clone(),
        allowed_roots: autonomy_config
            .allowed_roots
            .iter()
            .map(|root| {
                let expanded = expand_user_path(root);
                if expanded.is_absolute() {
                    expanded
                } else {
                    workspace_dir.join(expanded)
                }
            })
            .collect(),
        max_actions_per_hour: autonomy_config.max_actions_per_hour,
        max_cost_per_day_cents: autonomy_config.max_cost_per_day_cents,
        require_approval_for_medium_risk: autonomy_config.require_approval_for_medium_risk,
        block_high_risk_commands: autonomy_config.block_high_risk_commands,
        shell_env_passthrough: autonomy_config.shell_env_passthrough.clone(),
        tracker: ActionTracker::new(),
    }
}

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::fs;

pub const USER_MODEL_CATALOG_FILE: &str = "model_catalog.json";

#[derive(Debug, Clone)]
pub struct ModelCatalogOverrideStatus {
    pub config_dir: PathBuf,
    pub catalog_path: PathBuf,
    pub exists: bool,
    pub active: bool,
}

pub async fn resolve_user_model_catalog_status() -> Result<ModelCatalogOverrideStatus> {
    let (config_dir, _) = crate::workspace_io::resolve_runtime_dirs_for_onboarding().await?;
    let catalog_path = user_model_catalog_path(&config_dir);
    Ok(ModelCatalogOverrideStatus {
        config_dir,
        exists: catalog_path.exists(),
        active: synapse_domain::config::model_catalog::runtime_model_catalog_override_active(),
        catalog_path,
    })
}

pub fn user_model_catalog_path(config_dir: &Path) -> PathBuf {
    config_dir.join(USER_MODEL_CATALOG_FILE)
}

pub async fn install_runtime_model_catalog_override_if_present() -> Result<Option<PathBuf>> {
    let status = resolve_user_model_catalog_status().await?;
    install_runtime_model_catalog_override_from_path(&status.catalog_path).await
}

pub async fn install_runtime_model_catalog_override_from_dir(
    config_dir: &Path,
) -> Result<Option<PathBuf>> {
    let catalog_path = user_model_catalog_path(config_dir);
    install_runtime_model_catalog_override_from_path(&catalog_path).await
}

async fn install_runtime_model_catalog_override_from_path(
    catalog_path: &Path,
) -> Result<Option<PathBuf>> {
    if !catalog_path.exists() {
        return Ok(None);
    }

    let payload = fs::read_to_string(catalog_path).await.with_context(|| {
        format!(
            "failed to read user model catalog override {}",
            catalog_path.display()
        )
    })?;

    synapse_domain::config::model_catalog::install_runtime_model_catalog_override_json(&payload)
        .with_context(|| {
            format!(
                "failed to install user model catalog override {}",
                catalog_path.display()
            )
        })?;

    Ok(Some(catalog_path.to_path_buf()))
}

pub async fn init_user_model_catalog(force: bool) -> Result<PathBuf> {
    let status = resolve_user_model_catalog_status().await?;

    if status.exists && !force {
        anyhow::bail!(
            "user model catalog already exists at {} (use --force to overwrite)",
            status.catalog_path.display()
        );
    }

    fs::create_dir_all(&status.config_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create config directory {}",
                status.config_dir.display()
            )
        })?;

    fs::write(
        &status.catalog_path,
        synapse_domain::config::model_catalog::bundled_model_catalog_json(),
    )
    .await
    .with_context(|| {
        format!(
            "failed to write user model catalog {}",
            status.catalog_path.display()
        )
    })?;

    Ok(status.catalog_path)
}

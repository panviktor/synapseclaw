//! File-backed run recipe store.

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::fs;
use std::path::{Path, PathBuf};
use synapse_domain::domain::run_recipe::RunRecipe;
use synapse_domain::ports::run_recipe_store::RunRecipeStorePort;

pub struct FileRunRecipeStore {
    path: PathBuf,
    recipes: RwLock<Vec<RunRecipe>>,
}

impl FileRunRecipeStore {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let recipes = if path.exists() {
            let bytes = fs::read(&path)
                .with_context(|| format!("failed to read run recipe store {}", path.display()))?;
            if bytes.is_empty() {
                Vec::new()
            } else {
                serde_json::from_slice(&bytes).with_context(|| {
                    format!("failed to parse run recipe store {}", path.display())
                })?
            }
        } else {
            Vec::new()
        };

        Ok(Self {
            path,
            recipes: RwLock::new(recipes),
        })
    }

    fn persist(&self, recipes: &[RunRecipe]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create run recipe store directory {}",
                    parent.display()
                )
            })?;
        }
        let bytes =
            serde_json::to_vec_pretty(recipes).context("failed to serialize run recipes")?;
        fs::write(&self.path, bytes)
            .with_context(|| format!("failed to write run recipe store {}", self.path.display()))?;
        Ok(())
    }
}

impl RunRecipeStorePort for FileRunRecipeStore {
    fn list(&self, agent_id: &str) -> Vec<RunRecipe> {
        self.recipes
            .read()
            .iter()
            .filter(|recipe| recipe.agent_id == agent_id)
            .cloned()
            .collect()
    }

    fn list_recent(&self, agent_id: &str, min_updated_at: u64) -> Vec<RunRecipe> {
        self.recipes
            .read()
            .iter()
            .filter(|recipe| recipe.agent_id == agent_id && recipe.updated_at >= min_updated_at)
            .cloned()
            .collect()
    }

    fn upsert(&self, recipe: RunRecipe) -> Result<()> {
        let mut recipes = self.recipes.write();
        if let Some(existing) = recipes.iter_mut().find(|existing| {
            existing.agent_id == recipe.agent_id && existing.task_family == recipe.task_family
        }) {
            *existing = recipe;
        } else {
            recipes.push(recipe);
        }
        self.persist(&recipes)
    }

    fn remove(&self, agent_id: &str, task_family: &str) -> Result<()> {
        let mut recipes = self.recipes.write();
        recipes
            .retain(|recipe| !(recipe.agent_id == agent_id && recipe.task_family == task_family));
        self.persist(&recipes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(task_family: &str) -> RunRecipe {
        RunRecipe {
            agent_id: "agent".into(),
            task_family: task_family.into(),
            sample_request: "deploy the latest build".into(),
            summary: "Deployment succeeded".into(),
            tool_pattern: vec!["shell".into()],
            success_count: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn persists_recipes_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run_recipes.json");

        let store = FileRunRecipeStore::new(&path).unwrap();
        store.upsert(recipe("deploy")).unwrap();

        let reopened = FileRunRecipeStore::new(&path).unwrap();
        let recipes = reopened.list("agent");
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].task_family, "deploy");
    }

    #[test]
    fn removes_recipe_persistently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run_recipes.json");

        let store = FileRunRecipeStore::new(&path).unwrap();
        store.upsert(recipe("deploy")).unwrap();
        store.upsert(recipe("restart")).unwrap();
        store.remove("agent", "deploy").unwrap();

        let reopened = FileRunRecipeStore::new(&path).unwrap();
        let recipes = reopened.list("agent");
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].task_family, "restart");
    }

    #[test]
    fn list_recent_filters_by_updated_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run_recipes.json");

        let store = FileRunRecipeStore::new(&path).unwrap();
        store
            .upsert(RunRecipe {
                updated_at: 10,
                ..recipe("older")
            })
            .unwrap();
        store
            .upsert(RunRecipe {
                updated_at: 20,
                ..recipe("newer")
            })
            .unwrap();

        let recipes = store.list_recent("agent", 15);
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].task_family, "newer");
    }
}

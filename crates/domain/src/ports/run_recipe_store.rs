//! Port for storing reusable run recipes across turns and restarts.

use crate::domain::run_recipe::RunRecipe;
use anyhow::Result;

pub trait RunRecipeStorePort: Send + Sync {
    fn list(&self, agent_id: &str) -> Vec<RunRecipe>;
    fn list_recent(&self, agent_id: &str, min_updated_at: u64) -> Vec<RunRecipe> {
        self.list(agent_id)
            .into_iter()
            .filter(|recipe| recipe.updated_at >= min_updated_at)
            .collect()
    }
    fn upsert(&self, recipe: RunRecipe) -> Result<()>;
    fn remove(&self, agent_id: &str, task_family: &str) -> Result<()>;
    fn get(&self, agent_id: &str, task_family: &str) -> Option<RunRecipe> {
        self.list(agent_id)
            .into_iter()
            .find(|recipe| recipe.task_family == task_family)
    }
}

pub struct InMemoryRunRecipeStore {
    recipes: parking_lot::RwLock<Vec<RunRecipe>>,
}

impl InMemoryRunRecipeStore {
    pub fn new() -> Self {
        Self {
            recipes: parking_lot::RwLock::new(Vec::new()),
        }
    }
}

impl Default for InMemoryRunRecipeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RunRecipeStorePort for InMemoryRunRecipeStore {
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
        Ok(())
    }

    fn remove(&self, agent_id: &str, task_family: &str) -> Result<()> {
        self.recipes
            .write()
            .retain(|recipe| !(recipe.agent_id == agent_id && recipe.task_family == task_family));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(task_family: &str) -> RunRecipe {
        RunRecipe {
            agent_id: "agent".into(),
            task_family: task_family.into(),
            lineage_task_families: vec![task_family.into()],
            sample_request: "deploy the latest build".into(),
            summary: "Check staging logs, then deploy".into(),
            tool_pattern: vec!["shell".into()],
            success_count: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn upsert_and_get() {
        let store = InMemoryRunRecipeStore::new();
        store.upsert(recipe("deploy")).unwrap();
        assert_eq!(store.get("agent", "deploy").unwrap().task_family, "deploy");
    }

    #[test]
    fn list_is_agent_scoped() {
        let store = InMemoryRunRecipeStore::new();
        store.upsert(recipe("deploy")).unwrap();
        store
            .upsert(RunRecipe {
                agent_id: "other".into(),
                ..recipe("restart")
            })
            .unwrap();
        assert_eq!(store.list("agent").len(), 1);
    }

    #[test]
    fn remove_deletes_only_matching_recipe() {
        let store = InMemoryRunRecipeStore::new();
        store.upsert(recipe("deploy")).unwrap();
        store.upsert(recipe("restart")).unwrap();

        store.remove("agent", "deploy").unwrap();

        let recipes = store.list("agent");
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].task_family, "restart");
    }

    #[test]
    fn list_recent_filters_by_updated_at() {
        let store = InMemoryRunRecipeStore::new();
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

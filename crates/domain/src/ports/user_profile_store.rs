//! Port for durable dynamic user profiles.

use crate::domain::user_profile::UserProfile;
use anyhow::Result;
use std::collections::HashMap;

pub trait UserProfileStorePort: Send + Sync {
    fn load(&self, user_key: &str) -> Option<UserProfile>;
    fn list(&self) -> Vec<(String, UserProfile)>;
    fn upsert(&self, user_key: &str, profile: UserProfile) -> Result<()>;
    fn remove(&self, user_key: &str) -> Result<bool>;
}

pub struct InMemoryUserProfileStore {
    profiles: parking_lot::RwLock<HashMap<String, UserProfile>>,
}

impl InMemoryUserProfileStore {
    pub fn new() -> Self {
        Self {
            profiles: parking_lot::RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryUserProfileStore {
    fn default() -> Self {
        Self::new()
    }
}

impl UserProfileStorePort for InMemoryUserProfileStore {
    fn load(&self, user_key: &str) -> Option<UserProfile> {
        self.profiles.read().get(user_key).cloned()
    }

    fn list(&self) -> Vec<(String, UserProfile)> {
        let mut items = self
            .profiles
            .read()
            .iter()
            .map(|(key, profile)| (key.clone(), profile.clone()))
            .collect::<Vec<_>>();
        items.sort_by(|left, right| left.0.cmp(&right.0));
        items
    }

    fn upsert(&self, user_key: &str, profile: UserProfile) -> Result<()> {
        self.profiles.write().insert(user_key.to_string(), profile);
        Ok(())
    }

    fn remove(&self, user_key: &str) -> Result<bool> {
        Ok(self.profiles.write().remove(user_key).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn upsert_and_load_profile() {
        let store = InMemoryUserProfileStore::new();
        store
            .upsert("matrix:alice", {
                let mut profile = UserProfile::default();
                profile.set("project_alias", json!("Borealis"));
                profile
            })
            .unwrap();
        assert_eq!(
            store
                .load("matrix:alice")
                .and_then(|p| p.get_text("project_alias")),
            Some("Borealis".into())
        );
    }

    #[test]
    fn remove_reports_existence() {
        let store = InMemoryUserProfileStore::new();
        store
            .upsert("matrix:alice", UserProfile::default())
            .unwrap();
        assert!(store.remove("matrix:alice").unwrap());
        assert!(!store.remove("matrix:alice").unwrap());
    }

    #[test]
    fn list_profiles_returns_sorted_entries() {
        let store = InMemoryUserProfileStore::new();
        store.upsert("b", UserProfile::default()).unwrap();
        store.upsert("a", UserProfile::default()).unwrap();

        let keys = store
            .list()
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["a", "b"]);
    }
}

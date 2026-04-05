//! Port for durable structured user profiles.

use crate::domain::user_profile::UserProfile;
use anyhow::Result;
use std::collections::HashMap;

pub trait UserProfileStorePort: Send + Sync {
    fn load(&self, user_key: &str) -> Option<UserProfile>;
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

    #[test]
    fn upsert_and_load_profile() {
        let store = InMemoryUserProfileStore::new();
        store
            .upsert(
                "matrix:alice",
                UserProfile {
                    timezone: Some("Europe/Berlin".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            store.load("matrix:alice").and_then(|p| p.timezone),
            Some("Europe/Berlin".into())
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
}

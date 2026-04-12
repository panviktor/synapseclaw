//! File-backed user profile store.

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use synapse_domain::domain::user_profile::UserProfile;
use synapse_domain::ports::user_profile_store::UserProfileStorePort;

pub struct FileUserProfileStore {
    path: PathBuf,
    profiles: RwLock<HashMap<String, UserProfile>>,
}

impl FileUserProfileStore {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let profiles = if path.exists() {
            let bytes = fs::read(&path)
                .with_context(|| format!("failed to read user profile store {}", path.display()))?;
            if bytes.is_empty() {
                HashMap::new()
            } else {
                serde_json::from_slice(&bytes).with_context(|| {
                    format!("failed to parse user profile store {}", path.display())
                })?
            }
        } else {
            HashMap::new()
        };

        Ok(Self {
            path,
            profiles: RwLock::new(profiles),
        })
    }

    fn persist(&self, profiles: &HashMap<String, UserProfile>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create user profile store directory {}",
                    parent.display()
                )
            })?;
        }
        let bytes =
            serde_json::to_vec_pretty(profiles).context("failed to serialize user profiles")?;
        fs::write(&self.path, bytes).with_context(|| {
            format!("failed to write user profile store {}", self.path.display())
        })?;
        Ok(())
    }
}

impl UserProfileStorePort for FileUserProfileStore {
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
        let mut profiles = self.profiles.write();
        profiles.insert(user_key.to_string(), profile);
        self.persist(&profiles)
    }

    fn remove(&self, user_key: &str) -> Result<bool> {
        let mut profiles = self.profiles.write();
        let removed = profiles.remove(user_key).is_some();
        if removed {
            self.persist(&profiles)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn persists_profiles_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user_profiles.json");

        let store = FileUserProfileStore::new(&path).unwrap();
        store
            .upsert("matrix:alice", {
                let mut profile = UserProfile::default();
                profile.set("local_timezone", json!("Europe/Berlin"));
                profile
            })
            .unwrap();

        let reopened = FileUserProfileStore::new(&path).unwrap();
        assert_eq!(
            reopened
                .load("matrix:alice")
                .and_then(|profile| profile.get_text("local_timezone")),
            Some("Europe/Berlin".into())
        );
    }
}

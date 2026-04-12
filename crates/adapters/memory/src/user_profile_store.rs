//! SurrealDB-backed dynamic user profile store.
//!
//! The table is schemaless by design: one record per runtime user key, with an
//! arbitrary `facts` object. This keeps user state in the shared memory DB
//! without turning profile facts into fixed SQL-like columns.

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use synapse_domain::domain::user_profile::UserProfile;
use synapse_domain::ports::user_profile_store::UserProfileStorePort;

pub struct SurrealUserProfileStore {
    db: Arc<Surreal<Db>>,
}

impl SurrealUserProfileStore {
    pub fn new(db: Arc<Surreal<Db>>) -> Self {
        Self { db }
    }

    async fn load_async(db: Arc<Surreal<Db>>, user_key: String) -> Result<Option<UserProfile>> {
        let mut resp = db
            .query("SELECT user_key, facts FROM user_profile WHERE user_key = $user_key LIMIT 1")
            .bind(("user_key", user_key))
            .await
            .context("load user profile")?;
        let rows: Vec<Value> = resp.take(0).context("parse user profile row")?;
        Ok(rows.first().map(row_to_profile))
    }

    async fn list_async(db: Arc<Surreal<Db>>) -> Result<Vec<(String, UserProfile)>> {
        let mut resp = db
            .query("SELECT user_key, facts FROM user_profile ORDER BY user_key ASC")
            .await
            .context("list user profiles")?;
        let rows: Vec<Value> = resp.take(0).context("parse user profile rows")?;
        Ok(rows
            .iter()
            .map(|row| (json_str(row, "user_key"), row_to_profile(row)))
            .collect())
    }

    async fn upsert_async(
        db: Arc<Surreal<Db>>,
        user_key: String,
        profile: UserProfile,
    ) -> Result<()> {
        db.query(
            "IF (SELECT count() FROM user_profile WHERE user_key = $user_key GROUP ALL)[0].count > 0 {
                UPDATE user_profile SET facts = $facts, updated_at = time::now() WHERE user_key = $user_key
            } ELSE {
                CREATE user_profile SET user_key = $user_key, facts = $facts, created_at = time::now(), updated_at = time::now()
            };",
        )
        .bind(("user_key", user_key))
        .bind(("facts", profile.facts))
        .await
        .context("upsert user profile")?;
        Ok(())
    }

    async fn remove_async(db: Arc<Surreal<Db>>, user_key: String) -> Result<bool> {
        let existed = Self::load_async(Arc::clone(&db), user_key.clone())
            .await?
            .is_some();
        if existed {
            db.query("DELETE FROM user_profile WHERE user_key = $user_key")
                .bind(("user_key", user_key))
                .await
                .context("delete user profile")?;
        }
        Ok(existed)
    }

    fn block_on<T>(&self, future: impl Future<Output = Result<T>> + Send + 'static) -> Result<T>
    where
        T: Send + 'static,
    {
        block_on_user_profile_store(future)
    }
}

impl UserProfileStorePort for SurrealUserProfileStore {
    fn load(&self, user_key: &str) -> Option<UserProfile> {
        self.block_on(Self::load_async(Arc::clone(&self.db), user_key.to_string()))
            .map_err(|error| {
                tracing::warn!(%error, user_key, "surreal user profile load failed");
                error
            })
            .ok()
            .flatten()
    }

    fn list(&self) -> Vec<(String, UserProfile)> {
        self.block_on(Self::list_async(Arc::clone(&self.db)))
            .map_err(|error| {
                tracing::warn!(%error, "surreal user profile list failed");
                error
            })
            .unwrap_or_default()
    }

    fn upsert(&self, user_key: &str, profile: UserProfile) -> Result<()> {
        self.block_on(Self::upsert_async(
            Arc::clone(&self.db),
            user_key.to_string(),
            profile,
        ))
    }

    fn remove(&self, user_key: &str) -> Result<bool> {
        self.block_on(Self::remove_async(
            Arc::clone(&self.db),
            user_key.to_string(),
        ))
    }
}

fn row_to_profile(row: &Value) -> UserProfile {
    let facts = row
        .get("facts")
        .and_then(Value::as_object)
        .map(|facts| {
            facts
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    UserProfile { facts }
}

fn json_str(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn block_on_user_profile_store<T>(
    future: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if matches!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        ) {
            return tokio::task::block_in_place(|| handle.block_on(future));
        }

        return std::thread::spawn(move || run_user_profile_store_future(future))
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("user profile store worker panicked")));
    }

    run_user_profile_store_future(future)
}

fn run_user_profile_store_future<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build user profile store runtime")?
        .block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::engine::local::SurrealKv;

    #[tokio::test(flavor = "multi_thread")]
    async fn persists_dynamic_profile_in_surreal() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Surreal::new::<SurrealKv>(tmp.path().join("profile.surreal"))
            .await
            .unwrap();
        db.use_ns("synapseclaw").use_db("memory").await.unwrap();
        db.query(include_str!("surrealdb_schema.surql"))
            .await
            .unwrap();

        let store = SurrealUserProfileStore::new(Arc::new(db));
        let mut profile = UserProfile::default();
        profile.set("weather_city", serde_json::json!("Berlin"));
        store.upsert("web:abc", profile).unwrap();

        let loaded = store.load("web:abc").unwrap();
        assert_eq!(loaded.get_text("weather_city").as_deref(), Some("Berlin"));
        assert_eq!(store.list().len(), 1);
        assert!(store.remove("web:abc").unwrap());
        assert!(store.load("web:abc").is_none());
    }
}

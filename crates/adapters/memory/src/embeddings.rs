use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::HashMap;
use synapse_domain::domain::memory::EmbeddingProfile;

/// Trait for embedding providers — convert text to vectors
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Provider name
    fn name(&self) -> &str;

    /// Embedding dimensions
    fn dimensions(&self) -> usize;

    /// Retrieval calibration profile for this embedding family/model.
    fn profile(&self) -> EmbeddingProfile;

    /// Embed a batch of texts into vectors
    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;

    /// Embed a single text
    async fn embed_one(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let mut results = self.embed(&[text]).await?;
        results
            .pop()
            .ok_or_else(|| anyhow::anyhow!("Empty embedding result"))
    }

    /// Embed a user query with model-aware calibration.
    async fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let profile = self.profile();
        let prepared = prepare_embedding_text(text, profile.query_prefix.as_deref());
        let mut embedding = self.embed_one(&prepared).await?;
        if profile.normalize_output {
            normalize_embedding(&mut embedding);
        }
        Ok(embedding)
    }

    /// Embed a stored document/chunk with model-aware calibration.
    async fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let profile = self.profile();
        let prepared = prepare_embedding_text(text, profile.document_prefix.as_deref());
        let mut embedding = self.embed_one(&prepared).await?;
        if profile.normalize_output {
            normalize_embedding(&mut embedding);
        }
        Ok(embedding)
    }
}

// ── Disabled provider ────────────────────────────────────────

pub struct NoopEmbedding;

#[async_trait]
impl EmbeddingProvider for NoopEmbedding {
    fn name(&self) -> &str {
        "none"
    }

    fn dimensions(&self) -> usize {
        0
    }

    fn profile(&self) -> EmbeddingProfile {
        EmbeddingProfile::default()
    }

    async fn embed(&self, _texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(Vec::new())
    }
}

/// Default base URL for known embedding providers.
pub fn default_base_url_for_provider(provider: &str) -> Option<String> {
    match provider.to_lowercase().as_str() {
        "openai" => Some("https://api.openai.com/v1".to_string()),
        "openrouter" => Some("https://openrouter.ai/api/v1".to_string()),
        _ => None,
    }
}

// ── OpenAI-compatible embedding provider ─────────────────────

pub struct OpenAiEmbedding {
    base_url: String,
    api_key: String,
    model: String,
    dims: usize,
    profile: EmbeddingProfile,
}

impl OpenAiEmbedding {
    pub fn new(
        base_url: &str,
        api_key: &str,
        model: &str,
        dims: usize,
        profile: EmbeddingProfile,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            dims,
            profile,
        }
    }

    fn http_client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }

    fn has_explicit_api_path(&self) -> bool {
        let Ok(url) = reqwest::Url::parse(&self.base_url) else {
            return false;
        };

        let path = url.path().trim_end_matches('/');
        !path.is_empty() && path != "/"
    }

    fn has_embeddings_endpoint(&self) -> bool {
        let Ok(url) = reqwest::Url::parse(&self.base_url) else {
            return false;
        };

        url.path().trim_end_matches('/').ends_with("/embeddings")
    }

    fn embeddings_url(&self) -> String {
        if self.has_embeddings_endpoint() {
            return self.base_url.clone();
        }

        if self.has_explicit_api_path() {
            format!("{}/embeddings", self.base_url)
        } else {
            format!("{}/v1/embeddings", self.base_url)
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbedding {
    fn name(&self) -> &str {
        "openai"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn profile(&self) -> EmbeddingProfile {
        self.profile.clone()
    }

    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let resp = self
            .http_client()
            .post(self.embeddings_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Embedding API error {status}: {text}");
        }

        let json: serde_json::Value = resp.json().await?;
        let data = json
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow::anyhow!("Invalid embedding response: missing 'data'"))?;

        let mut embeddings = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| anyhow::anyhow!("Invalid embedding item"))?;

            #[allow(clippy::cast_possible_truncation)]
            let vec: Vec<f32> = embedding
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();

            embeddings.push(vec);
        }

        Ok(embeddings)
    }
}

// ── LlamaCpp provider (local llama-server) ───────────────────

/// Embedding provider for local llama.cpp server.
///
/// Expects llama-server running with `--embedding` flag, e.g.:
/// `llama-server -m nomic-embed-text-v1.5.Q8_0.gguf --embedding --port 8081`
///
/// Uses the OpenAI-compatible `/v1/embeddings` endpoint.
pub struct LlamaCppEmbedding {
    inner: OpenAiEmbedding,
}

impl LlamaCppEmbedding {
    /// Create a new local embedder.
    ///
    /// `url` is the llama-server base URL, e.g. `http://127.0.0.1:8081`.
    /// `model` is the model name passed in the request body (informational).
    /// `dims` is the embedding dimension (e.g. 768 for nomic-embed-text).
    pub fn new(url: &str, model: &str, dims: usize, profile: EmbeddingProfile) -> Self {
        Self {
            // llama-server doesn't require an API key
            inner: OpenAiEmbedding::new(url, "no-key-needed", model, dims, profile),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for LlamaCppEmbedding {
    fn name(&self) -> &str {
        "llama.cpp"
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    fn profile(&self) -> EmbeddingProfile {
        self.inner.profile()
    }

    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        self.inner.embed(texts).await
    }
}

// ── Cached embedding provider (LRU wrapper) ─────────────────

/// Wraps any EmbeddingProvider with an in-memory LRU cache.
pub struct CachedEmbeddingProvider {
    inner: Box<dyn EmbeddingProvider>,
    cache: Mutex<HashMap<String, Vec<f32>>>,
    max_entries: usize,
}

impl CachedEmbeddingProvider {
    pub fn new(inner: Box<dyn EmbeddingProvider>, max_entries: usize) -> Self {
        Self {
            inner,
            cache: Mutex::new(HashMap::new()),
            max_entries,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for CachedEmbeddingProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    fn profile(&self) -> EmbeddingProfile {
        self.inner.profile()
    }

    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        let mut uncached_texts = Vec::new();
        let mut uncached_indices = Vec::new();

        // Check cache for each text.
        {
            let cache = self.cache.lock();
            for (i, text) in texts.iter().enumerate() {
                if let Some(cached) = cache.get(*text) {
                    results.push(cached.clone());
                } else {
                    results.push(vec![]); // placeholder
                    uncached_texts.push(*text);
                    uncached_indices.push(i);
                }
            }
        }

        // Fetch uncached embeddings.
        if !uncached_texts.is_empty() {
            let fresh = self.inner.embed(&uncached_texts).await?;
            let mut cache = self.cache.lock();

            // Evict oldest entries if cache is full.
            while cache.len() + fresh.len() > self.max_entries && !cache.is_empty() {
                if let Some(key) = cache.keys().next().cloned() {
                    cache.remove(&key);
                }
            }

            for (idx, embedding) in uncached_indices.into_iter().zip(fresh.into_iter()) {
                if let Some(text) = texts.get(idx) {
                    cache.insert((*text).to_string(), embedding.clone());
                }
                results[idx] = embedding;
            }
        }

        Ok(results)
    }
}

pub fn resolve_embedding_profile(
    provider: &str,
    model: &str,
    dims: usize,
) -> Option<EmbeddingProfile> {
    synapse_domain::config::model_catalog::embedding_profile(provider, model, dims)
}

fn prepare_embedding_text(text: &str, prefix: Option<&str>) -> String {
    match prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}{text}"),
        _ => text.to_string(),
    }
}

fn normalize_embedding(embedding: &mut Vec<f32>) {
    let norm = embedding
        .iter()
        .map(|value| {
            let v = *value as f64;
            v * v
        })
        .sum::<f64>()
        .sqrt();
    if norm <= f64::EPSILON {
        return;
    }
    for value in embedding.iter_mut() {
        *value = (*value as f64 / norm) as f32;
    }
}

// ── Factory ──────────────────────────────────────────────────

pub fn create_embedding_provider(
    provider: &str,
    api_key: Option<&str>,
    model: &str,
    dims: usize,
) -> Box<dyn EmbeddingProvider> {
    let Some(profile) = resolve_embedding_profile(provider, model, dims) else {
        return Box::new(NoopEmbedding);
    };
    match provider {
        "openai" => {
            let Some(key) = api_key else {
                return Box::new(NoopEmbedding);
            };
            Box::new(OpenAiEmbedding::new(
                "https://api.openai.com",
                key,
                model,
                dims,
                profile,
            ))
        }
        "openrouter" => {
            let Some(key) = api_key else {
                return Box::new(NoopEmbedding);
            };
            Box::new(OpenAiEmbedding::new(
                "https://openrouter.ai/api/v1",
                key,
                model,
                dims,
                profile,
            ))
        }
        // Local llama.cpp server: "llama.cpp" or "llama.cpp:http://host:port"
        name if name == "llama.cpp" || name.starts_with("llama.cpp:") => {
            let url = if name == "llama.cpp" {
                "http://127.0.0.1:8081"
            } else {
                match name
                    .strip_prefix("llama.cpp:")
                    .filter(|url| !url.is_empty())
                {
                    Some(url) => url,
                    None => return Box::new(NoopEmbedding),
                }
            };
            Box::new(LlamaCppEmbedding::new(url, model, dims, profile))
        }
        name if name.starts_with("custom:") => {
            let Some(base_url) = name.strip_prefix("custom:").filter(|url| !url.is_empty()) else {
                return Box::new(NoopEmbedding);
            };
            let Some(key) = api_key else {
                return Box::new(NoopEmbedding);
            };
            Box::new(OpenAiEmbedding::new(base_url, key, model, dims, profile))
        }
        _ => Box::new(NoopEmbedding),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::domain::memory::EmbeddingDistanceMetric;

    fn test_profile(provider_family: &str, model: &str, dims: usize) -> EmbeddingProfile {
        EmbeddingProfile {
            profile_id: format!("{provider_family}:{model}:{dims}"),
            provider_family: provider_family.to_string(),
            model_id: model.to_string(),
            dimensions: dims,
            distance_metric: EmbeddingDistanceMetric::Cosine,
            normalize_output: dims > 0,
            query_prefix: None,
            document_prefix: None,
            supports_multilingual: false,
            supports_code: false,
            recommended_chunk_chars: 900,
            recommended_top_k: 8,
        }
    }

    #[test]
    fn noop_name() {
        let p = NoopEmbedding;
        assert_eq!(p.name(), "none");
        assert_eq!(p.dimensions(), 0);
    }

    #[tokio::test]
    async fn noop_embed_returns_empty() {
        let p = NoopEmbedding;
        let result = p.embed(&["hello"]).await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn factory_none() {
        let p = create_embedding_provider("none", None, "model", 1536);
        assert_eq!(p.name(), "none");
    }

    #[test]
    fn factory_openai() {
        let p = create_embedding_provider("openai", Some("key"), "text-embedding-3-small", 1536);
        assert_eq!(p.name(), "openai");
        assert_eq!(p.dimensions(), 1536);
    }

    #[test]
    fn factory_openrouter() {
        let p = create_embedding_provider(
            "openrouter",
            Some("sk-or-test"),
            "openai/text-embedding-3-small",
            1536,
        );
        assert_eq!(p.name(), "openai"); // uses OpenAiEmbedding internally
        assert_eq!(p.dimensions(), 1536);
    }

    #[tokio::test]
    async fn cached_provider_returns_same_result() {
        // Use a disabled provider wrapped in cache — noop returns empty,
        // so we just check no panics and correct structure.
        let inner = Box::new(NoopEmbedding);
        let cached = CachedEmbeddingProvider::new(inner, 100);
        assert_eq!(cached.name(), "none");
        assert_eq!(cached.dimensions(), 0);
        let result = cached.embed(&["hello"]).await.unwrap();
        // NoopEmbedding returns empty vec for any input
        assert!(result.is_empty() || result[0].is_empty());
    }

    #[test]
    fn factory_llamacpp_default_url() {
        let p = create_embedding_provider("llama.cpp", None, "multilingual-e5-small", 384);
        assert_eq!(p.name(), "llama.cpp");
        assert_eq!(p.dimensions(), 384);
    }

    #[test]
    fn factory_llamacpp_custom_url() {
        let p = create_embedding_provider(
            "llama.cpp:http://10.0.0.5:9090",
            None,
            "multilingual-e5-small",
            384,
        );
        assert_eq!(p.name(), "llama.cpp");
        assert_eq!(p.dimensions(), 384);
    }

    #[test]
    fn factory_custom_url_without_catalog_profile_is_disabled() {
        let p = create_embedding_provider("custom:http://localhost:1234", None, "model", 768);
        assert_eq!(p.name(), "none");
        assert_eq!(p.dimensions(), 0);
    }

    // ── Edge cases ───────────────────────────────────────────────

    #[tokio::test]
    async fn noop_embed_one_returns_error() {
        let p = NoopEmbedding;
        // embed returns empty vec → pop() returns None → error
        let result = p.embed_one("hello").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn noop_embed_empty_batch() {
        let p = NoopEmbedding;
        let result = p.embed(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn noop_embed_multiple_texts() {
        let p = NoopEmbedding;
        let result = p.embed(&["a", "b", "c"]).await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn factory_empty_string_returns_noop() {
        let p = create_embedding_provider("", None, "model", 1536);
        assert_eq!(p.name(), "none");
    }

    #[test]
    fn factory_unknown_provider_returns_noop() {
        let p = create_embedding_provider("cohere", None, "model", 1536);
        assert_eq!(p.name(), "none");
    }

    #[test]
    fn factory_custom_empty_url() {
        let p = create_embedding_provider("custom:", None, "model", 768);
        assert_eq!(p.name(), "none");
    }

    #[test]
    fn factory_openai_no_api_key() {
        let p = create_embedding_provider("openai", None, "text-embedding-3-small", 1536);
        assert_eq!(p.name(), "none");
        assert_eq!(p.dimensions(), 0);
    }

    #[test]
    fn openai_trailing_slash_stripped() {
        let p = OpenAiEmbedding::new(
            "https://api.openai.com/",
            "key",
            "model",
            1536,
            test_profile("openai", "model", 1536),
        );
        assert_eq!(p.base_url, "https://api.openai.com");
    }

    #[test]
    fn openai_dimensions_custom() {
        let p = OpenAiEmbedding::new(
            "http://localhost",
            "k",
            "m",
            384,
            test_profile("openai", "m", 384),
        );
        assert_eq!(p.dimensions(), 384);
    }

    #[test]
    fn embeddings_url_openrouter() {
        let p = OpenAiEmbedding::new(
            "https://openrouter.ai/api/v1",
            "key",
            "openai/text-embedding-3-small",
            1536,
            resolve_embedding_profile("openrouter", "openai/text-embedding-3-small", 1536)
                .expect("catalog profile"),
        );
        assert_eq!(
            p.embeddings_url(),
            "https://openrouter.ai/api/v1/embeddings"
        );
    }

    #[test]
    fn embeddings_url_standard_openai() {
        let p = OpenAiEmbedding::new(
            "https://api.openai.com",
            "key",
            "model",
            1536,
            test_profile("openai", "model", 1536),
        );
        assert_eq!(p.embeddings_url(), "https://api.openai.com/v1/embeddings");
    }

    #[test]
    fn embeddings_url_base_with_v1_no_duplicate() {
        let p = OpenAiEmbedding::new(
            "https://api.example.com/v1",
            "key",
            "model",
            1536,
            test_profile("openai", "model", 1536),
        );
        assert_eq!(p.embeddings_url(), "https://api.example.com/v1/embeddings");
    }

    #[test]
    fn embeddings_url_non_v1_api_path_uses_raw_suffix() {
        let p = OpenAiEmbedding::new(
            "https://api.example.com/api/coding/v3",
            "key",
            "model",
            1536,
            test_profile("custom", "model", 1536),
        );
        assert_eq!(
            p.embeddings_url(),
            "https://api.example.com/api/coding/v3/embeddings"
        );
    }

    #[test]
    fn embeddings_url_custom_full_endpoint() {
        let p = OpenAiEmbedding::new(
            "https://my-api.example.com/api/v2/embeddings",
            "key",
            "model",
            1536,
            test_profile("custom", "model", 1536),
        );
        assert_eq!(
            p.embeddings_url(),
            "https://my-api.example.com/api/v2/embeddings"
        );
    }

    #[test]
    fn profile_builder_uses_catalog_for_e5_prefixes() {
        let profile = resolve_embedding_profile("llama.cpp", "multilingual-e5-small", 384)
            .expect("catalog profile");
        assert_eq!(profile.query_prefix.as_deref(), Some("query: "));
        assert_eq!(profile.document_prefix.as_deref(), Some("passage: "));
        assert!(profile.normalize_output);
    }

    #[test]
    fn profile_builder_uses_catalog_for_qwen_embedding_calibration() {
        let profile = resolve_embedding_profile("openrouter", "qwen/qwen3-embedding-4b", 2560)
            .expect("catalog profile");
        assert_eq!(profile.provider_family, "openrouter");
        assert!(profile.supports_multilingual);
        assert!(profile.supports_code);
        assert_eq!(profile.recommended_top_k, 10);
    }

    #[test]
    fn profile_builder_does_not_infer_capabilities_from_model_substrings() {
        assert!(
            resolve_embedding_profile("openrouter", "qwen-like-unknown-embedding", 2560).is_none()
        );
    }
}

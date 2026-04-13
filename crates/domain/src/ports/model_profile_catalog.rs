use crate::config::schema::ModelFeature;
use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogModelProfileSource {
    CachedProviderCatalog,
    BundledCatalog,
    LocalOverrideCatalog,
    AdapterFallback,
}

pub fn catalog_model_profile_source_name(source: CatalogModelProfileSource) -> &'static str {
    match source {
        CatalogModelProfileSource::CachedProviderCatalog => "cached_provider_catalog",
        CatalogModelProfileSource::BundledCatalog => "bundled_catalog",
        CatalogModelProfileSource::LocalOverrideCatalog => "local_override_catalog",
        CatalogModelProfileSource::AdapterFallback => "adapter_fallback",
    }
}

/// Cached or discovered provider:model metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogModelProfile {
    pub context_window_tokens: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub features: Vec<ModelFeature>,
    pub source: Option<CatalogModelProfileSource>,
    pub observed_at_unix: Option<u64>,
}

/// Typed profile observation derived from a provider context-limit failure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextLimitProfileObservation {
    pub observed_context_window_tokens: Option<usize>,
    pub requested_context_tokens: Option<usize>,
}

/// Typed provider:model profile metadata discovered outside the static catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelProfileObservation {
    pub context_window_tokens: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub features: Vec<ModelFeature>,
}

/// Port for best-effort provider:model profile lookup.
///
/// Adapters can implement this using cached provider catalogs, live model
/// discovery, or curated metadata tables. The domain remains agnostic about
/// where the metadata came from.
pub trait ModelProfileCatalogPort: Send + Sync {
    fn lookup_model_profile(&self, provider: &str, model: &str) -> Option<CatalogModelProfile>;

    fn record_model_profile_observation(
        &self,
        provider: &str,
        model: &str,
        observation: ModelProfileObservation,
    ) -> Result<()>;

    fn record_context_limit_observation(
        &self,
        _provider: &str,
        _model: &str,
        _observation: ContextLimitProfileObservation,
    ) -> Result<()> {
        Ok(())
    }
}

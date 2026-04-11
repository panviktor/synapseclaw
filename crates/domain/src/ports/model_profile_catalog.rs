use crate::config::schema::ModelFeature;

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

/// Port for best-effort provider:model profile lookup.
///
/// Adapters can implement this using cached provider catalogs, live model
/// discovery, or curated metadata tables. The domain remains agnostic about
/// where the metadata came from.
pub trait ModelProfileCatalogPort: Send + Sync {
    fn lookup_model_profile(&self, provider: &str, model: &str) -> Option<CatalogModelProfile>;
}

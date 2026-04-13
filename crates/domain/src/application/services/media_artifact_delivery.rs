use anyhow::{anyhow, bail, Result};

use crate::ports::provider::{MediaArtifact, MediaArtifactLocator};

pub fn artifact_delivery_uri<'a>(transport: &str, artifact: &'a MediaArtifact) -> Result<&'a str> {
    let target = match &artifact.locator {
        MediaArtifactLocator::Uri { uri } => uri.as_str(),
        MediaArtifactLocator::ProviderFile { file } => file.uri.as_deref().ok_or_else(|| {
            anyhow!(
                "{transport} cannot deliver provider file artifact without a URI: provider={} file_id={}",
                file.provider,
                file.file_id
            )
        })?,
    }
    .trim();

    if target.is_empty() {
        bail!("{transport} cannot deliver media artifact with an empty URI");
    }

    Ok(target)
}

pub fn strip_media_artifact_markers(content: &str, artifacts: &[MediaArtifact]) -> String {
    let mut cleaned = content.to_string();
    for marker in artifacts.iter().filter_map(MediaArtifact::marker) {
        cleaned = cleaned.replace(&marker, "");
    }
    cleaned
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

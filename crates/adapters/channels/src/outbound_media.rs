use anyhow::{Context, Result};
use base64::Engine as _;
use synapse_domain::ports::provider::MediaArtifact;

pub struct OutboundMediaUpload {
    pub bytes: Vec<u8>,
    pub file_name: String,
    pub mime_type: String,
}

pub async fn resolve_outbound_media_artifact(
    client: &reqwest::Client,
    artifact: &MediaArtifact,
    uri: &str,
    fallback_file_name: &str,
) -> Result<OutboundMediaUpload> {
    resolve_outbound_media_uri(
        client,
        uri,
        artifact.label.as_deref(),
        artifact.mime_type.as_deref(),
        fallback_file_name,
    )
    .await
}

pub async fn resolve_outbound_media_uri(
    client: &reqwest::Client,
    uri: &str,
    label: Option<&str>,
    mime_type: Option<&str>,
    fallback_file_name: &str,
) -> Result<OutboundMediaUpload> {
    let uri = uri.trim();
    if uri.is_empty() {
        anyhow::bail!("cannot deliver media artifact with an empty URI");
    }

    if uri.starts_with("data:") {
        return data_uri_upload(uri, label, mime_type, fallback_file_name);
    }

    if uri.starts_with("http://") || uri.starts_with("https://") {
        return http_uri_upload(client, uri, label, mime_type, fallback_file_name).await;
    }

    local_uri_upload(uri, label, mime_type, fallback_file_name).await
}

pub fn is_remote_media_uri(uri: &str) -> bool {
    let uri = uri.trim();
    uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("data:")
}

pub fn local_media_path(uri: &str) -> Option<std::path::PathBuf> {
    let uri = uri.trim();
    if is_remote_media_uri(uri) {
        return None;
    }
    Some(
        uri.strip_prefix("file://")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(uri)),
    )
}

fn data_uri_upload(
    uri: &str,
    label: Option<&str>,
    mime_type: Option<&str>,
    fallback_file_name: &str,
) -> Result<OutboundMediaUpload> {
    let data = uri
        .strip_prefix("data:")
        .ok_or_else(|| anyhow::anyhow!("invalid data URI artifact"))?;
    let (meta, payload) = data
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("invalid data URI artifact: missing payload"))?;
    let mut parts = meta.split(';');
    let detected_mime_type = parts
        .next()
        .filter(|part| !part.trim().is_empty())
        .unwrap_or("application/octet-stream");
    let is_base64 = parts.any(|part| part.eq_ignore_ascii_case("base64"));
    if !is_base64 {
        anyhow::bail!("only base64 data URI media artifacts are supported");
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .context("failed to decode data URI media artifact")?;
    if bytes.is_empty() {
        anyhow::bail!("cannot deliver empty data URI media artifact");
    }

    Ok(OutboundMediaUpload {
        bytes,
        file_name: preferred_file_name(label, None, fallback_file_name),
        mime_type: mime_type
            .unwrap_or(detected_mime_type)
            .trim()
            .to_string(),
    })
}

async fn http_uri_upload(
    client: &reqwest::Client,
    uri: &str,
    label: Option<&str>,
    mime_type: Option<&str>,
    fallback_file_name: &str,
) -> Result<OutboundMediaUpload> {
    let response = client
        .get(uri)
        .send()
        .await
        .with_context(|| format!("failed to fetch outbound media artifact {uri}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|error| format!("<failed to read response body: {error}>"));
        anyhow::bail!("failed to fetch outbound media artifact {uri} ({status}): {body}");
    }

    let header_mime_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("failed to read outbound media artifact {uri}"))?
        .to_vec();
    if bytes.is_empty() {
        anyhow::bail!("cannot deliver empty outbound media artifact {uri}");
    }

    Ok(OutboundMediaUpload {
        bytes,
        file_name: preferred_file_name(label, file_name_from_http_uri(uri).as_deref(), fallback_file_name),
        mime_type: mime_type
            .map(str::to_string)
            .or(header_mime_type)
            .unwrap_or_else(|| "application/octet-stream".to_string()),
    })
}

async fn local_uri_upload(
    uri: &str,
    label: Option<&str>,
    mime_type: Option<&str>,
    fallback_file_name: &str,
) -> Result<OutboundMediaUpload> {
    let path = local_media_path(uri).expect("local_uri_upload is only called for local URIs");
    if !path.is_file() {
        anyhow::bail!(
            "cannot deliver media artifact URI as a local upload: {}",
            uri
        );
    }

    let bytes = tokio::fs::read(&path)
        .await
        .with_context(|| format!("failed to read outbound media artifact {}", path.display()))?;
    if bytes.is_empty() {
        anyhow::bail!("cannot deliver empty outbound media artifact {}", path.display());
    }

    let file_name = path.file_name().and_then(|name| name.to_str());
    let detected_mime_type = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| mime_guess::from_ext(ext).first_raw())
        .unwrap_or("application/octet-stream");

    Ok(OutboundMediaUpload {
        bytes,
        file_name: preferred_file_name(label, file_name, fallback_file_name),
        mime_type: mime_type.unwrap_or(detected_mime_type).trim().to_string(),
    })
}

fn file_name_from_http_uri(uri: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(uri).ok()?;
    parsed
        .path_segments()?
        .rev()
        .find(|segment| !segment.trim().is_empty())
        .map(ToString::to_string)
}

fn preferred_file_name(label: Option<&str>, detected: Option<&str>, fallback: &str) -> String {
    label
        .or(detected)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .trim()
        .to_string()
}

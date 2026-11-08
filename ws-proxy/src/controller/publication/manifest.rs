//! Resolve source manifests inside publication attempts. Failed resolution is retried
//! by Apalis and becomes PreparationFailed only after the attempt limit.
use std::{path::Path, time::Duration};

use serde_json::Value;
use suntitle_core::kubernetes::resources::{ExtensionSourceType, SuntitleExtensionSpec};
use tokio::io::AsyncReadExt;
use super::transport::invalid;

/// Controller source failures propagated into publication retries and status.
type ManifestError = suntitle_core::kubernetes::Error;

/// Maximum manifest bytes accepted from any transport.
const MANIFEST_LIMIT: u64 = 1024 * 1024;
/// Root manifest filenames, in lookup order.
const MANIFEST_NAMES: [&str; 3] = ["manifest.yaml", "manifest.yml", "manifest.json"];

/// Resolve a source-only registration using controller transport configuration.
/// The returned Git snapshot MUST be retained until payload packaging completes.
pub(super) async fn resolve(
    spec: &SuntitleExtensionSpec,
    sources: &super::Sources,
    cancellation: &oci_artifact::CancellationToken,
) -> Result<(SuntitleExtensionSpec, Option<super::git::Snapshot>), ManifestError> {
    let mut snapshot = None;
    let bytes = match spec.source_type {
        ExtensionSourceType::Local => read_directory(Path::new(&spec.source)).await?,
        ExtensionSourceType::Http => read_http(&spec.source, sources, cancellation).await?,
        ExtensionSourceType::Git => {
            let fetched = sources.fetch_git(&spec.source, cancellation).await?;
            let bytes = fetched.read_manifest(MANIFEST_NAMES, MANIFEST_LIMIT)?;
            snapshot = Some(fetched);
            bytes
        }
        ExtensionSourceType::Oci => bounded(spec.manifest.as_ref()
            .filter(|manifest| !manifest.trim().is_empty())
            .ok_or_else(|| invalid("OCI registration requires separate manifest YAML or JSON contents."))?
            .as_bytes().to_vec())?,
    };
    Ok((decode_manifest(&bytes, spec.source_type.clone(), &spec.source)?, snapshot))
}

/// Read one regular manifest with a hard byte limit.
async fn read_file(path: &Path) -> Result<Vec<u8>, ManifestError> {
    if !tokio::fs::metadata(path).await.map_err(|_| invalid("Cannot inspect the extension manifest."))?.is_file() {
        return Err(invalid("The extension manifest MUST be a regular file."));
    }
    let file = tokio::fs::File::open(path).await.map_err(|_| invalid("Cannot read the extension manifest. Check its path and permissions."))?;
    let mut bytes = Vec::new();
    file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes).await.map_err(|_| invalid("Cannot read the extension manifest."))?;
    bounded(bytes)
}

/// Reject oversized manifests before parsing YAML or JSON.
fn bounded(bytes: Vec<u8>) -> Result<Vec<u8>, ManifestError> {
    if bytes.len() as u64 > MANIFEST_LIMIT {
        return Err(invalid("Extension manifest exceeds 1 MiB."));
    }
    Ok(bytes)
}

/// Find a manifest in an absolute extension directory.
async fn read_directory(directory: &Path) -> Result<Vec<u8>, ManifestError> {
    let directory = super::transport::local_directory(directory)?;
    for filename in MANIFEST_NAMES {
        let path = directory.join(filename);
        match tokio::fs::metadata(&path).await {
            Ok(_) => return read_file(&path).await,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(invalid("Cannot access the extension manifest. Check directory permissions.")),
        }
    }
    Err(invalid("No manifest.yaml, manifest.yml, or manifest.json found in the extension directory."))
}

/// Read a sibling manifest beside the HTTP payload consumed by publication.
async fn read_http(source: &str, sources: &super::Sources, cancellation: &oci_artifact::CancellationToken) -> Result<Vec<u8>, ManifestError> {
    let url = super::transport::http_url(source)?;
    if url.path().ends_with('/') {
        return Err(invalid("HTTP source MUST point to the extension payload file, with a sibling manifest."));
    }
    for filename in MANIFEST_NAMES {
        let mut location = url.join(filename).map_err(|_| invalid("Invalid manifest URL."))?;
        location.set_query(url.query());
        let mut response = sources.http_get(location, Some(Duration::from_secs(20)), cancellation).await
            .map_err(|_| invalid("Cannot fetch the extension manifest. Check URL and connectivity."))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND { continue; }
        if !response.status().is_success() { return Err(invalid("The manifest server rejected the request.")); }
        let mut bytes = Vec::new();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err("publication cancelled".into()),
                result = response.chunk() => result.map_err(|_| invalid("Manifest transfer failed."))?,
            };
            let Some(chunk) = chunk else { break; };
            if bytes.len() + chunk.len() > MANIFEST_LIMIT as usize { return Err(invalid("Extension manifest exceeds 1 MiB.")); }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(bytes);
    }
    Err(invalid("No sibling manifest.yaml, manifest.yml, or manifest.json found at the HTTP source."))
}

/// Accept resource, create-request, or standalone spec manifests; ignore status and source overrides.
pub(super) fn decode_manifest(bytes: &[u8], source_type: ExtensionSourceType, source: &str) -> Result<SuntitleExtensionSpec, ManifestError> {
    let document: Value = serde_yaml::from_slice(bytes).map_err(|_| invalid("Extension manifest MUST contain valid YAML or JSON."))?;
    if let Some(kind) = document.get("kind") {
        if kind != "SuntitleExtension" { return Err(invalid("Manifest kind MUST be SuntitleExtension.")); }
    }
    let mut spec = document.get("spec").unwrap_or(&document).clone();
    let fields = spec.as_object_mut().ok_or_else(|| invalid("Manifest MUST contain an extension spec object."))?;
    fields.insert("sourceType".into(), serde_json::to_value(source_type).unwrap());
    fields.insert("source".into(), Value::String(source.to_owned()));
    fields.remove("refreshToken");
    let spec: SuntitleExtensionSpec = serde_json::from_value(spec).map_err(|_| invalid("Manifest requires a displayName and an adapter with valid execution. Check any settings and arguments."))?;
    if spec.display_name.trim().is_empty() || spec.display_name.chars().count() > 128 || spec.max_attempts == 0 {
        return Err(invalid("Manifest displayName MUST have 1–128 characters and maxAttempts MUST be positive."));
    }
    if spec.adapter.is_none() { return Err(invalid("Manifest requires an adapter.")); }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Exercise the production resolver while retaining only its decoded configuration.
    async fn resolve_spec(spec: &SuntitleExtensionSpec, config: &super::super::PublicationConfig, cancellation: &oci_artifact::CancellationToken) -> Result<SuntitleExtensionSpec, ManifestError> {
        let sources = super::super::Sources::new(config.clone(), cancellation.clone())?;
        super::resolve(spec, &sources, cancellation).await.map(|(spec, _)| spec)
    }

    /// Source-only intent has no adapter until the controller reads its manifest.
    fn source_spec(source_type: ExtensionSourceType, source: String) -> SuntitleExtensionSpec {
        serde_json::from_value(serde_json::json!({"displayName":"Pending", "sourceType":source_type,"source":source})).unwrap()
    }

    /// A portable manifest omits deployment-specific source fields.
    fn manifest() -> Value {
        json!({"displayName":"Hello", "adapter":{ "execution":{"runtime":"python","command":"./hello.py"}, "settings":{"optional":{"default":null},"required":{"required":true}}, "arguments":[{"flag":"--server","source":"runtime.websocketUrl"}]}})
    }

    /// OCI manifests come from registration contents, without contacting the artifact registry.
    #[tokio::test]
    async fn resolves_separate_oci_manifest_without_registry_access() {
        let mut spec = source_spec(ExtensionSourceType::Oci, "unreachable.invalid/binary:latest".into());
        let config = super::super::tests::config();
        let cancellation = oci_artifact::CancellationToken::new();
        assert!(resolve_spec(&spec, &config, &cancellation).await.unwrap_err().to_string().contains("separate manifest"));
        for contents in [serde_yaml::to_string(&manifest()).unwrap(), serde_json::to_string(&manifest()).unwrap()] {
            spec.manifest = Some(contents);
            let resolved = resolve_spec(&spec, &config, &cancellation).await.unwrap();
            assert_eq!(resolved.display_name, "Hello");
            assert_eq!(resolved.source, spec.source);
            assert_eq!(resolved.source_type, ExtensionSourceType::Oci);
            assert!(resolved.adapter.is_some());
        }
        spec.manifest = Some("invalid: [yaml".into());
        assert!(resolve_spec(&spec, &config, &cancellation).await.unwrap_err().to_string().contains("valid YAML or JSON"));
        spec.manifest = Some("x".repeat(MANIFEST_LIMIT as usize + 1));
        assert!(resolve_spec(&spec, &config, &cancellation).await.unwrap_err().to_string().contains("1 MiB"));
    }

    /// Each supported envelope retains adapter declarations while the request owns source fields.
    #[test]
    fn resolves_manifest_envelopes_without_overriding_the_selected_source() {
        for document in [
            manifest(),
            json!({"name":"hello","displayName":"Hello","adapter":manifest()["adapter"]}),
            json!({"name":"hello","spec":manifest()}),
            json!({"kind":"SuntitleExtension","metadata":{"name":"hello"},"spec":manifest(),"status":{"artifactReference":"old"}}),
        ] {
            let bytes = serde_yaml::to_string(&document).unwrap();
            let registered = decode_manifest(bytes.as_bytes(), ExtensionSourceType::Local, "/mounted/hello").unwrap();
            let spec = serde_json::to_value(registered).unwrap();
            assert_eq!(spec["displayName"], "Hello");
            assert_eq!(spec["source"], "/mounted/hello");
            assert_eq!(spec["sourceType"], "local");
            assert_eq!(spec["adapter"]["settings"]["optional"]["default"], Value::Null);
            assert!(spec["adapter"]["settings"]["optional"].get("default").is_some());
            assert_eq!(spec["adapter"]["arguments"][0]["source"], "runtime.websocketUrl");
            assert_eq!(spec["maxAttempts"], 3);
        }
    }

    /// Invalid configuration MUST fail before artifact publication.
    #[test]
    fn rejects_invalid_manifests_and_untrusted_git_protocols() {
        for document in [json!({}), json!({"kind":"SuntitleHost","spec":manifest()}), json!({"displayName":"Hello","adapter":{"execution":{"command":""}}}), json!({"displayName":"Hello","maxAttempts":0,"adapter":manifest()["adapter"]})] {
            assert!(decode_manifest(&serde_json::to_vec(&document).unwrap(), ExtensionSourceType::Git, "https://example.com/repo.git").is_err());
        }
        for source in ["file:///etc", "ssh://example.com/repo", "https://user:secret@example.com/repo", "--upload-pack=example"] {
            assert!(super::super::transport::http_url(source).is_err());
        }
        assert!(bounded(vec![b' '; MANIFEST_LIMIT as usize + 1]).is_err());
    }

    /// Directory lookup supports JSON fallback and reports missing or malformed manifests.
    #[tokio::test]
    async fn discovers_local_manifests_and_rejects_bad_sources() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().to_string_lossy().into_owned();
        assert!(read_directory(directory.path()).await.is_err());
        tokio::fs::write(directory.path().join("manifest.json"), serde_json::to_vec(&manifest()).unwrap()).await.unwrap();
        let resolved = resolve_spec(&source_spec(ExtensionSourceType::Local, source.clone()), &super::super::tests::config(), &oci_artifact::CancellationToken::new()).await.unwrap();
        assert_eq!(resolved.display_name, "Hello");
        assert_eq!(resolved.source, source);
        tokio::fs::write(directory.path().join("manifest.yaml"), "not: [valid").await.unwrap();
        assert!(resolve_spec(&source_spec(ExtensionSourceType::Local, source), &super::super::tests::config(), &oci_artifact::CancellationToken::new()).await.is_err());
        assert!(read_directory(Path::new("relative")).await.is_err());
    }

    /// HTTP discovery uses sibling manifests, supports fallback, and rejects remote failures.
    #[tokio::test]
    async fn discovers_http_sibling_manifest() {
        let reply = manifest();
        let route = warp::path!("payload" / "manifest.json").map(move || warp::reply::json(&reply));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(warp::serve(route).incoming(listener).run());
        let source = format!("http://{address}/payload/hello.py");
        let resolved = resolve_spec(&source_spec(ExtensionSourceType::Http, source.clone()), &super::super::tests::config(), &oci_artifact::CancellationToken::new()).await.unwrap();
        assert_eq!(resolved.source, source);
        assert_eq!(resolved.display_name, "Hello");
        assert!(resolve_spec(&source_spec(ExtensionSourceType::Http, format!("http://{address}/missing.py")), &super::super::tests::config(), &oci_artifact::CancellationToken::new()).await.is_err());
        server.abort();
    }
    use super::super::git::tests::{git as git_fixture, serve as git_http_server};
    use warp::Filter;

    /// Git discovery reads root manifests from real smart HTTP without a working checkout.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovers_git_manifest_over_http() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("repo");
        std::fs::create_dir(&work).unwrap();
        git_fixture(&work, &["init", "-b", "main"]);
        std::fs::write(work.join("manifest.yaml"), serde_yaml::to_string(&manifest()).unwrap()).unwrap();
        git_fixture(&work, &["add", "."]);
        git_fixture(&work, &["commit", "-m", "Add manifest"]);
        let (source, server) = git_http_server(root.path()).await;
        let cancellation = oci_artifact::CancellationToken::new();
        let sources = super::super::Sources::new(super::super::tests::config(), cancellation.clone()).unwrap();
        let (resolved, snapshot) = super::resolve(&source_spec(ExtensionSourceType::Git, source.clone()), &sources, &cancellation).await.unwrap();
        assert_eq!(resolved.display_name, "Hello");
        std::fs::write(work.join("manifest.yaml"), "displayName: Changed").unwrap();
        git_fixture(&work, &["add", "."]);
        git_fixture(&work, &["commit", "-m", "Change manifest"]);
        let pinned = snapshot.unwrap().read_manifest(MANIFEST_NAMES, MANIFEST_LIMIT).unwrap();
        assert_eq!(decode_manifest(&pinned, ExtensionSourceType::Git, &source).unwrap().display_name, "Hello");
        assert_eq!(resolved.source, source);
        assert_eq!(resolved.source_type, ExtensionSourceType::Git);
        server.abort();
    }

}

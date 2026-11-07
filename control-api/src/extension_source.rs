//! Resolve source manifests before persisting registration. Invalid or unreachable
//! manifests MUST NOT create resources. Publication and installation remain deferred.
use std::{path::Path, time::Duration};

use actix_web::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use suntitle_core::kubernetes::resources::{ExtensionSourceType, SuntitleExtensionSpec};
use tokio::io::AsyncReadExt;
use utoipa::ToSchema;

use crate::{api::errors::ApiError, store::ExtensionCreate};

/// Maximum manifest bytes accepted from any transport.
const MANIFEST_LIMIT: u64 = 1024 * 1024;
/// Root manifest filenames, in lookup order.
const MANIFEST_NAMES: [&str; 3] = ["manifest.yaml", "manifest.yml", "manifest.json"];

/// Registration location; all adapter fields come from the source manifest.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionRegistration {
    /// Transport used to read and subsequently publish the extension.
    pub source_type: ExtensionSourceType,
    /// Absolute server directory, HTTP(S) Git URL, OCI reference, or HTTP payload URL.
    #[schema(min_length = 1)]
    pub source: String,
}

impl ExtensionRegistration {
    /// Load a bounded manifest and override its source with the selected location.
    pub async fn resolve(self) -> Result<ExtensionCreate, ApiError> {
        if self.source.trim().is_empty() {
            return Err(invalid("Extension path or URL is required."));
        }
        if self.source_type == ExtensionSourceType::Oci {
            let bytes = read_oci(&self.source).await?;
            return decode_manifest(&bytes, self.source_type, &self.source);
        }
        let operation = async {
            let bytes = match self.source_type {
                ExtensionSourceType::Local => read_directory(Path::new(&self.source)).await?,
                ExtensionSourceType::Git => read_git(&self.source).await?,
                ExtensionSourceType::Http => read_http(&self.source).await?,
                ExtensionSourceType::Oci => read_oci(&self.source).await?,
            };
            decode_manifest(&bytes, self.source_type, &self.source)
        };
        tokio::time::timeout(Duration::from_secs(90), operation)
            .await
            .map_err(|_| invalid("Reading the extension manifest timed out."))?
    }
}

/// Return a client-readable failure without exposing credentials or manifest contents.
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::problem(StatusCode::UNPROCESSABLE_ENTITY, "Invalid extension source", message)
}

/// Read one regular manifest with a hard byte limit.
async fn read_file(path: &Path) -> Result<Vec<u8>, ApiError> {
    if !tokio::fs::metadata(path).await.map_err(|_| invalid("Cannot inspect the extension manifest."))?.is_file() {
        return Err(invalid("The extension manifest MUST be a regular file."));
    }
    let file = tokio::fs::File::open(path).await.map_err(|_| invalid("Cannot read the extension manifest. Check its path and permissions."))?;
    let mut bytes = Vec::new();
    file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes).await.map_err(|_| invalid("Cannot read the extension manifest."))?;
    bounded(bytes)
}

/// Reject oversized manifests before parsing YAML or JSON.
fn bounded(bytes: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    if bytes.len() as u64 > MANIFEST_LIMIT {
        return Err(invalid("Extension manifest exceeds 1 MiB."));
    }
    Ok(bytes)
}

/// Find a manifest in an absolute extension directory.
async fn read_directory(directory: &Path) -> Result<Vec<u8>, ApiError> {
    if !directory.is_absolute() || !directory.is_dir() {
        return Err(invalid("Local source MUST be an absolute directory accessible to the server."));
    }
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

/// Require HTTP(S) without credentials for remote manifest discovery.
fn http_url(source: &str) -> Result<reqwest::Url, ApiError> {
    let url = reqwest::Url::parse(source).map_err(|_| invalid("Source requires an HTTP(S) URL."))?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("Source requires HTTP(S) without embedded credentials. SSH Git URLs are unsupported."));
    }
    Ok(url)
}

/// Read a sibling manifest beside the HTTP payload consumed by publication.
async fn read_http(source: &str) -> Result<Vec<u8>, ApiError> {
    let url = http_url(source)?;
    if url.path().ends_with('/') {
        return Err(invalid("HTTP source MUST point to the extension payload file, with a sibling manifest."));
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().map_err(|_| invalid("Cannot initialize manifest transport."))?;
    for filename in MANIFEST_NAMES {
        let mut location = url.join(filename).map_err(|_| invalid("Invalid manifest URL."))?;
        location.set_query(url.query());
        let mut response = client.get(location).send().await.map_err(|_| invalid("Cannot fetch the extension manifest. Check URL and connectivity."))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND { continue; }
        if !response.status().is_success() { return Err(invalid("The manifest server rejected the request.")); }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| invalid("Manifest transfer failed."))? {
            if bytes.len() + chunk.len() > MANIFEST_LIMIT as usize { return Err(invalid("Extension manifest exceeds 1 MiB.")); }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(bytes);
    }
    Err(invalid("No sibling manifest.yaml, manifest.yml, or manifest.json found at the HTTP source."))
}

/// Fetch the default Git revision without executing repository code or checking out files.
async fn read_git(source: &str) -> Result<Vec<u8>, ApiError> {
    http_url(source)?;
    let directory = tempfile::tempdir().map_err(|_| invalid("Cannot create manifest staging directory."))?;
    let repository = directory.path().join("repository");
    let status = git_command().args(["clone", "--bare", "--depth=1", "--", source]).arg(&repository)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().await
        .map_err(|_| invalid("Cannot run Git to read the extension manifest."))?;
    if !status.success() { return Err(invalid("Cannot fetch Git source. Check repository access and URL.")); }
    for filename in MANIFEST_NAMES {
        let object = format!("HEAD:{filename}");
        let size = git_command().arg("--git-dir").arg(&repository).args(["cat-file", "-s", &object]).output().await.map_err(|_| invalid("Cannot inspect Git manifest."))?;
        if !size.status.success() { continue; }
        let length = String::from_utf8_lossy(&size.stdout).trim().parse::<u64>().map_err(|_| invalid("Invalid Git manifest size."))?;
        if length > MANIFEST_LIMIT { return Err(invalid("Extension manifest exceeds 1 MiB.")); }
        let output = git_command().arg("--git-dir").arg(&repository).args(["cat-file", "blob", &object]).output().await.map_err(|_| invalid("Cannot read Git manifest."))?;
        if !output.status.success() { return Err(invalid("Git manifest MUST be a regular file.")); }
        return bounded(output.stdout);
    }
    Err(invalid("No root manifest.yaml, manifest.yml, or manifest.json found in the Git source."))
}

/// Disable host Git helpers and interactive prompts. Dropped requests kill child processes.
fn git_command() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command.env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_TERMINAL_PROMPT", "0").kill_on_drop(true);
    command
}

/// Download OCI payloads to private staging and read their root manifest.
async fn read_oci(source: &str) -> Result<Vec<u8>, ApiError> {
    let directory = tempfile::tempdir().map_err(|_| invalid("Cannot create OCI manifest staging directory."))?;
    let downloader = oci_artifact::Downloader::new(oci_artifact::client::TransportOptions {
        plain_http: std::env::var("SUNTITLE_EXTENSION_REGISTRY_PLAIN_HTTP").as_deref() == Ok("true"),
        ..Default::default()
    }).map_err(|_| invalid("Cannot initialize OCI manifest transport."))?;
    let request = oci_artifact::DownloadRequest::new(source, directory.path());
    let token = request.cancellation.clone();
    let operation = downloader.download(request);
    tokio::pin!(operation);
    let result = tokio::select! {
        result = &mut operation => result,
        _ = tokio::time::sleep(Duration::from_secs(60)) => {
            token.cancel();
            let _ = operation.await;
            return Err(invalid("Reading the OCI manifest timed out."));
        }
    }.map_err(|_| invalid("Cannot read OCI source. Check reference, registry access, and transport settings."))?;
    read_directory(&result.payload_path).await
}

/// Accept resource, create-request, or standalone spec manifests; ignore status and source overrides.
fn decode_manifest(bytes: &[u8], source_type: ExtensionSourceType, source: &str) -> Result<ExtensionCreate, ApiError> {
    let document: Value = serde_yaml::from_slice(bytes).map_err(|_| invalid("Extension manifest MUST contain valid YAML or JSON."))?;
    if let Some(kind) = document.get("kind") {
        if kind != "SuntitleExtension" { return Err(invalid("Manifest kind MUST be SuntitleExtension.")); }
    }
    let name = document.pointer("/metadata/name").or_else(|| document.get("name")).and_then(Value::as_str).map(str::to_owned);
    let mut spec = document.get("spec").unwrap_or(&document).clone();
    let fields = spec.as_object_mut().ok_or_else(|| invalid("Manifest MUST contain an extension spec object."))?;
    fields.insert("sourceType".into(), serde_json::to_value(source_type).unwrap());
    fields.insert("source".into(), Value::String(source.to_owned()));
    fields.remove("refreshToken");
    let spec: SuntitleExtensionSpec = serde_json::from_value(spec).map_err(|_| invalid("Manifest requires a displayName and an adapter with valid type and execution. Check any settings and arguments."))?;
    if spec.display_name.trim().is_empty() || spec.display_name.chars().count() > 128 || spec.max_attempts == 0 {
        return Err(invalid("Manifest displayName MUST have 1–128 characters and maxAttempts MUST be positive."));
    }
    Ok(ExtensionCreate { name, spec })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A portable manifest omits deployment-specific source fields.
    fn manifest() -> Value {
        json!({"displayName":"Hello", "adapter":{"type":"translator", "execution":{"runtime":"python","command":"./hello.py"}, "settings":{"optional":{"default":null},"required":{"required":true}}, "arguments":[{"flag":"--server","source":"runtime.websocketUrl"}]}})
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
            let expected_name = document.pointer("/metadata/name").or_else(|| document.get("name")).and_then(Value::as_str);
            assert_eq!(registered.name.as_deref(), expected_name);
            let spec = serde_json::to_value(registered.spec).unwrap();
            assert_eq!(spec["displayName"], "Hello");
            assert_eq!(spec["source"], "/mounted/hello");
            assert_eq!(spec["sourceType"], "local");
            assert_eq!(spec["adapter"]["settings"]["optional"]["default"], Value::Null);
            assert!(spec["adapter"]["settings"]["optional"].get("default").is_some());
            assert_eq!(spec["adapter"]["arguments"][0]["source"], "runtime.websocketUrl");
            assert_eq!(spec["maxAttempts"], 3);
        }
    }

    /// Invalid configuration MUST fail before resource persistence.
    #[test]
    fn rejects_invalid_manifests_and_untrusted_git_protocols() {
        for document in [json!({}), json!({"kind":"SuntitleHost","spec":manifest()}), json!({"displayName":"Hello","adapter":{"type":"translator","execution":{"command":""}}}), json!({"displayName":"Hello","maxAttempts":0,"adapter":manifest()["adapter"]})] {
            assert!(decode_manifest(&serde_json::to_vec(&document).unwrap(), ExtensionSourceType::Git, "https://example.com/repo.git").is_err());
        }
        for source in ["file:///etc", "ssh://example.com/repo", "https://user:secret@example.com/repo", "--upload-pack=example"] {
            assert!(http_url(source).is_err());
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
        let resolved = ExtensionRegistration { source_type: ExtensionSourceType::Local, source: source.clone() }.resolve().await.unwrap();
        assert_eq!(resolved.spec.display_name, "Hello");
        assert_eq!(resolved.spec.source, source);
        tokio::fs::write(directory.path().join("manifest.yaml"), "not: [valid").await.unwrap();
        assert!(ExtensionRegistration { source_type: ExtensionSourceType::Local, source }.resolve().await.is_err());
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
        let resolved = ExtensionRegistration { source_type: ExtensionSourceType::Http, source: source.clone() }.resolve().await.unwrap();
        assert_eq!(resolved.spec.source, source);
        assert_eq!(resolved.spec.display_name, "Hello");
        assert!(read_http(&format!("http://{address}/missing.py")).await.is_err());
        server.abort();
    }
    use std::{io::Write, process::{Command, Stdio}};
    use warp::Filter;

    /// Run the Git reference implementation only to prepare test inputs.
    fn git_fixture(directory: &Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(directory)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    /// Serve Git's smart HTTP backend; no registry emulator or external service is needed.
    async fn git_http_server(root: &Path) -> (String, tokio::task::JoinHandle<()>) {
        let root = root.to_owned();
        let route = warp::method()
            .and(warp::path::full())
            .and(warp::query::raw().or(warp::any().map(String::new)).unify())
            .and(warp::header::optional::<String>("git-protocol"))
            .and(warp::body::bytes())
            .map(
                move |method: warp::http::Method,
                      path: warp::path::FullPath,
                      query: String,
                      protocol: Option<String>,
                      body: actix_web::web::Bytes| {
                    let mut child = Command::new("git")
                        .arg("http-backend")
                        .env("GIT_PROJECT_ROOT", &root)
                        .env("GIT_HTTP_EXPORT_ALL", "1")
                        .env("PATH_INFO", path.as_str())
                        .env("QUERY_STRING", query)
                        .env("REQUEST_METHOD", method.as_str())
                        .env("CONTENT_TYPE", "application/x-git-upload-pack-request")
                        .env("CONTENT_LENGTH", body.len().to_string())
                        .env("HTTP_GIT_PROTOCOL", protocol.unwrap_or_default())
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .spawn()
                        .unwrap();
                    child.stdin.take().unwrap().write_all(&body).unwrap();
                    let output = child.wait_with_output().unwrap();
                    assert!(output.status.success());
                    let split = output
                        .stdout
                        .windows(4)
                        .position(|bytes| bytes == b"\r\n\r\n")
                        .unwrap();
                    let headers = String::from_utf8_lossy(&output.stdout[..split]);
                    let media = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Type: "))
                        .unwrap();
                    warp::http::Response::builder()
                        .header("Content-Type", media)
                        .body(output.stdout[split + 4..].to_vec())
                        .unwrap()
                },
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        (
            format!("http://{address}/repo/.git"),
            tokio::spawn(warp::serve(route).incoming(listener).run()),
        )
    }


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
        let resolved = ExtensionRegistration { source_type: ExtensionSourceType::Git, source: source.clone() }.resolve().await.unwrap();
        assert_eq!(resolved.spec.display_name, "Hello");
        assert_eq!(resolved.spec.source, source);
        assert_eq!(resolved.spec.source_type, ExtensionSourceType::Git);
        server.abort();
    }

}

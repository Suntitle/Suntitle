//! API registration publishes each local fixture before Diamant installs and launches it.
//!
//! Node watches MUST observe installation pending before Running. Process evidence
//! confirms execution from downloaded files. Finish signaling precedes agent shutdown,
//! which reaps any remaining fixture processes.
mod support;

use futures_util::TryStreamExt;
use kube::api::{WatchEvent, WatchParams};
use oci_artifact::{CancellationToken, Publisher, RegistryAuth, client::TransportOptions};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use support::{
    ApiClient, ChildProcess, binary_directory, binary_path, fixture_python, wait_for_services,
};
use tokio::time::{sleep, timeout};
use uuid::Uuid;

/// Maximum time for publication, installation, or launch evidence.
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Read reviewable YAML and replace only the run-specific string values.
fn fixture(path: &Path, replacements: &[(&str, &str)]) -> Result<Value, Box<dyn Error>> {
    let mut value: Value = serde_yaml::from_str(&fs::read_to_string(path)?)?;
    substitute(&mut value, replacements);
    Ok(value)
}

/// Preserve YAML value types while substituting paths and identities in strings.
fn substitute(value: &mut Value, replacements: &[(&str, &str)]) {
    match value {
        Value::String(value) => {
            for (key, replacement) in replacements {
                *value = value.replace(key, replacement);
            }
        }
        Value::Array(values) => {
            for value in values {
                substitute(value, replacements);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                substitute(value, replacements);
            }
        }
        _ => {}
    }
}

/// Match current resource conditions; flat extension responses retain the observed status revision.
fn condition(resource: &Value, kind: &str, status: &str, reason: Option<&str>) -> bool {
    resource["status"]["conditions"]
        .as_array()
        .is_some_and(|conditions| {
            conditions.iter().any(|c| {
                c["type"] == kind
                    && c["status"] == status
                    && &c["observedGeneration"] == resource["metadata"].get("generation").unwrap_or(&resource["status"]["observedGeneration"])
                    && reason.is_none_or(|reason| c["reason"] == reason)
            })
        })
}

/// Watch from creation so fast installation transitions cannot be missed by polling.
async fn installed_then_running(
    namespace: &support::TestNamespace,
    created: &Value,
    reference: &Value,
    version: &str,
) -> Result<(), Box<dyn Error>> {
    let nodes = namespace.nodes();
    let name = created["name"]
        .as_str()
        .ok_or("node name missing")?;
    let stream = nodes
        .watch(
            &WatchParams::default().fields(&format!("metadata.name={name}")),
            version,
        )
        .await?;
    tokio::pin!(stream);
    timeout(ACTION_TIMEOUT, async {
        let mut installed = false;
        while let Some(event) = stream.try_next().await? {
            let node = match event {
                WatchEvent::Added(node) | WatchEvent::Modified(node) => serde_json::to_value(node)?,
                WatchEvent::Error(error) => {
                    return Err(format!("node watch failed: {error:?}").into());
                }
                _ => continue,
            };
            if node["status"]["phase"] == "Failed" {
                return Err(format!("node failed: {node}").into());
            }
            if condition(&node, "Installed", "True", Some("ExtensionInstalled")) {
                assert_eq!(&node["status"]["artifactReference"], reference);
                if node["status"]["phase"] == "Pending"
                    && condition(&node, "Ready", "False", Some("AwaitingLaunch"))
                {
                    installed = true;
                }
            }
            if node["status"]["phase"] == "Running" && condition(&node, "Ready", "True", None) {
                if !installed {
                    return Err(format!("Running preceded installation observation: {node}").into());
                }
                return Ok::<(), Box<dyn Error>>(());
            }
        }
        Err("node watch ended before Running".into())
    })
    .await
    .map_err(|_| format!("timed out watching installation and launch of {name}"))??;
    Ok(())
}

/// Exercise local and OCI YAML-defined extensions against real services and one assigned host.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    support::require_free_ports()?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data = root.join("examples/custom_node_e2e");
    let sources = data.join("extensions");
    let binaries = binary_directory()?;
    if !sources.join("native/bin/probe").is_file() {
        return Err(
            "native fixture missing; run just custom-node-fixtures before the harness".into(),
        );
    }
    let client = ApiClient::new()?;
    let registry =
        env::var("CUSTOM_NODE_E2E_REGISTRY").unwrap_or_else(|_| "127.0.0.1:18080".into());
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(format!("http://{registry}/v2/"))
        .send()
        .await?
        .error_for_status()?;
    let run = tempfile::Builder::new()
        .prefix("custom node e2e ")
        .tempdir()?;
    let kubeconfigs = run.path().join("kubeconfigs");
    fs::create_dir(&kubeconfigs)?;
    for identity in ["admin", "suntitle-api", "suntitle-proxy"] {
        let filename = format!("{identity}.kubeconfig");
        let config = fs::read_to_string(
            root.parent()
                .ok_or("repository root missing")?
                .join("pki")
                .join(&filename),
        )?;
        fs::write(
            kubeconfigs.join(filename),
            config.replace("https://127.0.0.1:6443", "https://127.0.0.1:16443"),
        )?;
    }
    let kube_client =
        suntitle_core::kubernetes::client_from_kubeconfig(&kubeconfigs.join("admin.kubeconfig"))
            .await
            .map_err(io::Error::other)?;
    timeout(ACTION_TIMEOUT, async {
        while kube_client.apiserver_version().await.is_err() {
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| "isolated Kubernetes API did not become ready")?;
    let namespace = support::TestNamespace::with_kubeconfigs(kubeconfigs)
        .await
        .map_err(io::Error::other)?;
    let artifacts = run.path().join("artifacts");
    let host_name = format!("custom-e2e-{}", Uuid::new_v4().simple());
    let replacements = [
        (
            "${SOURCE_ROOT}",
            sources.to_str().ok_or("source path is not UTF-8")?,
        ),
        ("${HOST_NAME}", &host_name),
        ("${REGISTRY}", &registry),
        (
            "${RUN_ROOT}",
            run.path().to_str().ok_or("run path is not UTF-8")?,
        ),
    ];
    let mut api = ChildProcess::spawn(
        "control_api",
        &mut namespace.command(binary_path(&binaries, "control_api"), "api"),
    )?;
    let mut proxy = ChildProcess::spawn(
        "ws-proxy",
        namespace
            .command(binary_path(&binaries, "ws-proxy"), "proxy")
            .arg("--extension-registry")
            .arg(format!("{registry}/custom-node-e2e/{host_name}"))
            .arg("--extension-registry-plain-http"),
    )?;
    wait_for_services(&mut [&mut api, &mut proxy]).await?;
    let mut diamant = ChildProcess::spawn(
        "diamant",
        Command::new(binary_path(&binaries, "diamant"))
            .args([
                "--cli",
                "--proxy-url",
                "ws://127.0.0.1:3030/register",
                "--host-name",
            ])
            .arg(&host_name)
            .args([
                "--host-ip",
                "192.0.2.30",
                "--health-interval-seconds",
                "1",
                "--artifact-dir",
            ])
            .arg(&artifacts)
            .arg("--artifact-plain-http")
            .env("UV_PYTHON", fixture_python()?)
            .env("UV_OFFLINE", "true")
            .env("UV_PYTHON_DOWNLOADS", "never"),
    )?;
    let host = client
        .wait_for_host(
            &host_name,
            "Online",
            ACTION_TIMEOUT,
            &mut [&mut proxy, &mut diamant],
        )
        .await?;
    let host_id = host["name"].as_str().ok_or("host name missing")?;
    let scenarios = fixture(&data.join("scenarios.yaml"), &[])?;
    let publisher = Publisher::new(TransportOptions {
        plain_http: true,
        ..Default::default()
    })?;
    let mut published_sources = BTreeMap::<String, String>::new();
    for scenario in scenarios.as_array().ok_or("scenarios MUST be an array")? {
        let name = scenario["name"].as_str().ok_or("scenario name missing")?;
        let extension = fixture(&data.join(format!("{name}-extension.yaml")), &replacements)?;
        let seeded = if let Some(seed) = scenario["seedFrom"].as_str() {
            let source = published_sources
                .get(seed)
                .ok_or("local seed artifact missing")?;
            Some(
                publisher
                    .republish(
                        source,
                        extension["source"]
                            .as_str()
                            .ok_or("OCI source missing")?,
                        &RegistryAuth::Anonymous,
                        &RegistryAuth::Anonymous,
                        &CancellationToken::new(),
                    )
                    .await?,
            )
        } else {
            None
        };
        client
            .request("POST", "/api/v1/extensions", Some(&extension), 201)
            .await?;
        let published = client
            .wait_for(
                &format!("/api/v1/extensions/{}", extension["name"].as_str().unwrap()),
                ACTION_TIMEOUT,
                |e| Ok(condition(e, "Ready", "True", None)),
            )
            .await?;
        let reference = &published["status"]["artifactReference"];
        assert!(
            reference
                .as_str()
                .is_some_and(|r| r.starts_with(&format!("{registry}/")) && r.contains("@sha256:"))
        );
        if let Some(seeded) = seeded {
            let mirrored = reference.as_str().ok_or("mirrored reference missing")?;
            assert_ne!(
                mirrored.split('@').next(),
                seeded.split('@').next(),
                "OCI source MUST be mirrored into a different repository"
            );
            assert_eq!(
                mirrored.split('@').nth(1),
                seeded.split('@').nth(1),
                "mirroring MUST preserve the manifest digest"
            );
        }
        published_sources.insert(
            name.to_owned(),
            reference
                .as_str()
                .ok_or("artifact reference missing")?
                .to_owned(),
        );
        let node = fixture(&data.join(format!("{name}-node.yaml")), &replacements)?;
        let version = namespace.nodes().list(&Default::default()).await?.metadata.resource_version
            .ok_or("node collection resourceVersion missing")?;
        let created = client
            .request(
                "POST",
                &format!("/api/v1/hosts/{host_id}/nodes"),
                Some(&node),
                201,
            )
            .await?;
        installed_then_running(&namespace, &created, reference, &version).await?;
        let report_path = run.path().join(format!("{name}.json"));
        let report = timeout(ACTION_TIMEOUT, async {
            loop {
                if let Ok(bytes) = fs::read(&report_path)
                    && let Ok(report) = serde_json::from_slice::<Value>(&bytes)
                {
                    break report;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| format!("missing process evidence for {name}"))?;
        let mut expected = fixture(&data.join("expectations.yaml"), &replacements)?;
        substitute(&mut expected, &[("${RUNTIME}", name)]);
        assert_eq!(report["args"], expected["arguments"]);
        let cwd = Path::new(report["cwd"].as_str().ok_or("cwd missing")?);
        assert!(
            cwd.starts_with(fs::canonicalize(&artifacts)?),
            "process did not run from installed payload: {report}"
        );
        let entry = if scenario["runtime"] == "executable" {
            &report["executable"]
        } else {
            &report["entry"]
        };
        assert_eq!(
            Path::new(entry.as_str().ok_or("entry missing")?),
            cwd.join(scenario["entry"].as_str().ok_or("expected entry missing")?)
        );
        if scenario["runtime"] == "pythonModule" {
            assert_eq!(report["module"], expected["module"]);
        }
        println!(
            "PASS: {name}: API registration -> publication -> installation -> process launch ({reference})"
        );
    }
    fs::write(run.path().join("finish"), b"")?;
    Ok(())
}

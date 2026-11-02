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
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

/// Maximum time for publication, installation, or launch evidence.
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Application process owned by this run. Shutdown MUST reap its children first.
struct Process {
    /// Live child, stopped and reaped when the guard is dropped.
    child: Child,
}

impl Process {
    /// Start a real application with diagnostics inherited by the test runner.
    fn start(command: &mut Command) -> io::Result<Self> {
        Ok(Self {
            child: command
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()?,
        })
    }
}

impl Drop for Process {
    /// Give Diamant time to stop supervised processes before forced cleanup.
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Ok(pid) = i32::try_from(self.child.id()) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            );
            for _ in 0..70 {
                if self.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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

/// Send a real control API request and retain error response bodies.
async fn request(
    client: &reqwest::Client,
    method: reqwest::Method,
    path: &str,
    body: Option<&Value>,
    expected: u16,
) -> Result<Value, Box<dyn Error>> {
    let mut request = client.request(method, format!("http://127.0.0.1:3031/api/v1{path}"));
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await?;
    let status = response.status().as_u16();
    let text = response.text().await?;
    if status != expected {
        return Err(format!("{path}: expected {expected}, got {status}: {text}").into());
    }
    Ok(serde_json::from_str(&text)?)
}

/// Wait for a matching API observation while retaining the last response for diagnostics.
async fn wait_api(
    client: &reqwest::Client,
    path: &str,
    predicate: impl Fn(&Value) -> bool,
) -> Result<Value, Box<dyn Error>> {
    let deadline = Instant::now() + ACTION_TIMEOUT;
    loop {
        let result = request(client, reqwest::Method::GET, path, None, 200).await;
        if let Ok(value) = &result
            && predicate(value)
        {
            return Ok(value.clone());
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {path}: {result:?}").into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// Match a condition only when it describes the resource's current generation.
fn condition(resource: &Value, kind: &str, status: &str, reason: Option<&str>) -> bool {
    resource["status"]["conditions"]
        .as_array()
        .is_some_and(|conditions| {
            conditions.iter().any(|c| {
                c["type"] == kind
                    && c["status"] == status
                    && c["observedGeneration"] == resource["metadata"]["generation"]
                    && reason.is_none_or(|reason| c["reason"] == reason)
            })
        })
}

/// Watch from creation so fast installation transitions cannot be missed by polling.
async fn installed_then_running(
    namespace: &support::TestNamespace,
    created: &Value,
    reference: &Value,
) -> Result<(), Box<dyn Error>> {
    let nodes = namespace.nodes();
    let name = created["metadata"]["name"]
        .as_str()
        .ok_or("node name missing")?;
    let version = created["metadata"]["resourceVersion"]
        .as_str()
        .ok_or("node resourceVersion missing")?;
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

/// Build the real services and retain a native fixture in its development source directory.
fn build(root: &Path, binaries: &Path, sources: &Path) -> Result<(), Box<dyn Error>> {
    let status = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "--locked", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .args(["--bins", "--example", "custom_node_probe", "--target-dir"])
        .arg(binaries.parent().ok_or("target directory missing")?)
        .status()?;
    if !status.success() {
        return Err("failed to build E2E applications".into());
    }
    fs::copy(
        binaries.join("examples/custom_node_probe"),
        sources.join("native/bin/probe"),
    )?;
    Ok(())
}

/// Exercise local and OCI YAML-defined extensions against real services and one assigned host.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    support::require_free_ports()?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data = root.join("examples/custom_node_e2e");
    let sources = data.join("extensions");
    let executable = env::current_exe()?;
    let binaries = executable
        .parent()
        .and_then(Path::parent)
        .ok_or("binary directory missing")?;
    build(&root, binaries, &sources)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let registry =
        env::var("CUSTOM_NODE_E2E_REGISTRY").unwrap_or_else(|_| "127.0.0.1:18080".into());
    client
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
        ws_proxy::kubernetes::client_from_kubeconfig(&kubeconfigs.join("admin.kubeconfig"))
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
    let _api = Process::start(&mut namespace.command(binaries.join("control_api"), "api"))?;
    let _proxy = Process::start(
        namespace
            .command(binaries.join("ws-proxy"), "proxy")
            .arg("--extension-registry")
            .arg(format!("{registry}/custom-node-e2e/{host_name}"))
            .arg("--extension-registry-plain-http"),
    )?;
    wait_api(&client, "/hosts", |_| true).await?;
    let python = Command::new("/usr/bin/python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()?;
    if !python.status.success() {
        return Err("fixture Python interpreter unavailable".into());
    }
    let _diamant = Process::start(
        Command::new(binaries.join("diamant"))
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
            .env("UV_PYTHON", String::from_utf8(python.stdout)?.trim())
            .env("UV_OFFLINE", "true")
            .env("UV_PYTHON_DOWNLOADS", "never"),
    )?;
    let hosts = wait_api(&client, "/hosts", |hosts| {
        hosts["items"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|h| h["spec"]["name"] == host_name && h["status"]["presence"] == "Online")
        })
    })
    .await?;
    let host = hosts["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["spec"]["name"] == host_name)
        .unwrap();
    let host_id = host["metadata"]["uid"].as_str().ok_or("host UID missing")?;
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
                        extension["spec"]["source"]
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
        request(
            &client,
            reqwest::Method::POST,
            "/extensions",
            Some(&extension),
            201,
        )
        .await?;
        let published = wait_api(
            &client,
            &format!("/extensions/{}", extension["name"].as_str().unwrap()),
            |e| condition(e, "Ready", "True", None),
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
        let created = request(
            &client,
            reqwest::Method::POST,
            &format!("/hosts/{host_id}/nodes"),
            Some(&node),
            201,
        )
        .await?;
        installed_then_running(&namespace, &created, reference).await?;
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

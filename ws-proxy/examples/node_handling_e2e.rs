//! Verify built-in node launch, process completion, and replay after service restart.
//! Recipes prepare binaries. Fixtures exit before disconnect; shared guards own application shutdown.
mod support;

#[cfg(not(unix))]
fn main() {
    eprintln!("node_handling_e2e currently requires a Unix host");
}

#[cfg(unix)]
mod unix {
    use std::collections::BTreeSet;
    use std::error::Error;
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    use serde_json::{Value, json};
    use tokio::time::{Instant, sleep};
    use uuid::Uuid;

    const ACTION_TIMEOUT: Duration = Duration::from_secs(10);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    use crate::support::{
        ApiClient, ChildProcess, binary_directory, binary_path, fixture_python, wait_for_services,
    };

    /// Run built-in node launch and replay assertions using prepared application binaries.
    pub async fn run() -> Result<(), Box<dyn Error>> {
        crate::support::require_free_ports()?;
        let binary_directory = binary_directory()?;
        let client = ApiClient::new()?;
        let namespace = crate::support::TestNamespace::new()
            .await
            .map_err(io::Error::other)?;
        let temporary_directory = tempfile::tempdir()?;
        let process_log = temporary_directory.path().join("node-processes.log");
        let project = temporary_directory.path().join("project with spaces");
        fs::create_dir(&project)?;
        fs::write(
            project.join("pyproject.toml"),
            "[project]\nname = \"embedded-uv-e2e\"\nversion = \"0.0.0\"\nrequires-python = \">=3.9\"\ndependencies = []\n",
        )?;
        let fixture = r#"import json, os, pathlib, sys, time
log = pathlib.Path(os.environ['FAKE_NODE_LOG'])
with log.open('a') as f:
    f.write(f'START\t{os.getpid()}\t{json.dumps(sys.argv)}\n')
deadline = time.monotonic() + 45
while not pathlib.Path(os.environ['FINISH_FILE']).exists() and time.monotonic() < deadline:
    time.sleep(.05)
with log.open('a') as f:
    f.write(f'STOP\t{os.getpid()}\n')
"#;
        fs::write(project.join("suntitle-listener.py"), fixture)?;
        fs::write(project.join("suntitle.py"), fixture)?;
        let (reference, _registry) = registry(&project).await?;
        let host_name = format!("node-e2e-{}", Uuid::new_v4().simple());

        let mut api = ChildProcess::spawn(
            "control_api",
            &mut namespace.command(binary_path(&binary_directory, "control_api"), "api"),
        )?;
        let mut proxy = ChildProcess::spawn(
            "ws-proxy",
            namespace
                .command(binary_path(&binary_directory, "ws-proxy"), "proxy")
                .env("DEFAULT_EXTENSIONS_REFERENCE", &reference),
        )?;
        wait_for_services(&mut [&mut api, &mut proxy]).await?;

        let mut diamant_command = Command::new(binary_path(&binary_directory, "diamant"));
        diamant_command
            .arg("--cli")
            .arg("--proxy-url")
            .arg("ws://127.0.0.1:3030/register")
            .arg("--host-name")
            .arg(&host_name)
            .arg("--host-ip")
            .arg("192.0.2.20")
            .arg("--health-interval-seconds")
            .arg("1")
            .env("UV_PYTHON", fixture_python()?)
            .env("UV_OFFLINE", "true")
            .env("UV_PYTHON_DOWNLOADS", "never")
            .env("FAKE_NODE_LOG", &process_log)
            .env("FINISH_FILE", project.join("finish"))
            .arg("--artifact-plain-http")
            .arg("--artifact-dir")
            .arg(temporary_directory.path().join("artifacts"));
        let mut diamant = ChildProcess::spawn("diamant", &mut diamant_command)?;

        let host_id =
            wait_for_host(&client, &host_name, "Online", &mut proxy, &mut diamant).await?;
        let audio = client
            .request(
                "POST",
                &format!("/api/v1/hosts/{host_id}/nodes"),
                Some(&audio_body(&host_name, "Studio microphone", 16_000)),
                201,
            )
            .await?;
        let audio_id = audio["name"]
            .as_str()
            .ok_or("missing node name")?
            .to_owned();
        let display = client
            .request(
                "POST",
                &format!("/api/v1/hosts/{host_id}/nodes"),
                Some(&display_body(&host_name, "Main display", 3)),
                201,
            )
            .await?;
        let display_id = display["name"]
            .as_str()
            .ok_or("missing node name")?
            .to_owned();

        wait_for_nodes(&client, 2, true, &mut proxy, &mut diamant).await?;
        wait_for_starts(&process_log, "suntitle-listener.py", 1).await?;
        wait_for_starts(&process_log, "suntitle.py", 1).await?;

        wait_for_node(&client, &audio_id, "Studio microphone", true).await?;
        wait_for_node(&client, &display_id, "Main display", true).await?;

        client
            .request(
                "POST",
                &format!("/api/v1/hosts/{host_id}/nodes"),
                Some(&display_body(&host_name, "Auxiliary display", 5)),
                201,
            )
            .await?;
        wait_for_nodes(&client, 3, true, &mut proxy, &mut diamant).await?;
        wait_for_starts(&process_log, "suntitle.py", 2).await?;

        // Fixtures MUST exit before testing disconnect and replay.
        fs::write(project.join("finish"), "")?;
        wait_for_all_processes_to_stop(&process_log).await?;
        diamant.terminate()?;
        diamant.wait_for_exit().await?;
        wait_for_nodes(&client, 3, false, &mut proxy, &mut diamant).await?;
        wait_for_host(&client, &host_name, "Offline", &mut proxy, &mut diamant).await?;
        wait_for_all_processes_to_stop(&process_log).await?;

        // Restart both independent services with the same Kubernetes namespace. The API
        // must retain desired nodes, and a fresh agent must replay them.
        drop(api);
        drop(proxy);
        api = ChildProcess::spawn(
            "control_api",
            &mut namespace.command(binary_path(&binary_directory, "control_api"), "api"),
        )?;
        proxy = ChildProcess::spawn(
            "ws-proxy",
            namespace
                .command(binary_path(&binary_directory, "ws-proxy"), "proxy")
                .env("DEFAULT_EXTENSIONS_REFERENCE", &reference),
        )?;
        wait_for_services(&mut [&mut api, &mut proxy]).await?;
        api.ensure_running()?;
        wait_for_nodes(&client, 3, false, &mut proxy, &mut diamant).await?;
        fs::remove_file(project.join("finish"))?;
        diamant = ChildProcess::spawn("diamant", &mut diamant_command)?;
        wait_for_nodes(&client, 3, true, &mut proxy, &mut diamant).await?;
        wait_for_starts(&process_log, "suntitle-listener.py", 2).await?;
        wait_for_starts(&process_log, "suntitle.py", 4).await?;
        wait_for_node(&client, &audio_id, "Studio microphone", true).await?;
        // Fixtures MUST exit before testing disconnect and replay.
        fs::write(project.join("finish"), "")?;
        wait_for_all_processes_to_stop(&process_log).await?;
        diamant.terminate()?;
        diamant.wait_for_exit().await?;
        wait_for_nodes(&client, 3, false, &mut proxy, &mut diamant).await?;
        wait_for_all_processes_to_stop(&process_log).await?;

        println!(
            "PASS: embedded uv node create/list/add, fixture completion, and service restart replay completed for host {host_id}"
        );
        Ok(())
    }

    /// Serve a default Python payload through the OCI API, independently of the host workspace.
    async fn registry(
        project: &Path,
    ) -> Result<(String, tokio::task::JoinHandle<()>), Box<dyn Error>> {
        use oci_artifact::constants::{
            ANNOTATION_UNPACK, IMAGE_LAYER_GZIP_MEDIA_TYPE, OCI_IMAGE_MEDIA_TYPE,
            ORG_OPENCONTAINERS_IMAGE_TITLE,
        };
        use sha2::{Digest, Sha256};
        use warp::Filter;
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        archive.append_dir_all("payload", project)?;
        let blob = archive.into_inner()?.finish()?;
        let digest = |bytes: &[u8]| format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
        let manifest = serde_json::to_vec(&json!({
            "schemaVersion":2,"mediaType":OCI_IMAGE_MEDIA_TYPE,
            "config":{"mediaType":"application/vnd.oci.empty.v1+json","digest":digest(b"{}"),"size":2},
            "layers":[{"mediaType":IMAGE_LAYER_GZIP_MEDIA_TYPE,"digest":digest(&blob),"size":blob.len(),
                "annotations":{(ORG_OPENCONTAINERS_IMAGE_TITLE):"payload",(ANNOTATION_UNPACK):"true"}}]
        }))?;
        let route = warp::path::full().map(move |path: warp::path::FullPath| {
            let (body, media) = if path.as_str().contains("/manifests/") {
                (manifest.clone(), OCI_IMAGE_MEDIA_TYPE)
            } else if path.as_str().contains("/blobs/") {
                (blob.clone(), "application/octet-stream")
            } else {
                (b"{}".to_vec(), "application/json")
            };
            warp::http::Response::builder()
                .header("Content-Type", media)
                .body(body)
                .unwrap()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let reference = format!("{}/default-extensions:fixture", listener.local_addr()?);
        Ok((
            reference,
            tokio::spawn(warp::serve(route).incoming(listener).run()),
        ))
    }

    fn node_body(host: &str, name: &str, settings: Value) -> Value {
        json!({
            "displayName": name, "description": null, "enabled": true,
            "hostRef": {"name": host}, "inputRef": null, "restartToken": null,
            "settings": settings
        })
    }

    fn audio_body(host: &str, name: &str, sample_rate: u32) -> Value {
        node_body(
            host,
            name,
            json!({"audio_source": {
                "sampleRateHz": sample_rate, "channels": 1,
                "encoding": "pcm_s16le", "deviceName": null
            }}),
        )
    }

    fn display_body(host: &str, name: &str, max_lines: u8) -> Value {
        node_body(
            host,
            name,
            json!({"display": {
                "maxLines": max_lines, "fontScale": 1.0,
                "showOriginal": false, "theme": "dark"
            }}),
        )
    }

    /// Resolve a host resource name after shared presence polling.
    async fn wait_for_host(
        client: &ApiClient,
        name: &str,
        status: &str,
        proxy: &mut ChildProcess,
        diamant: &mut ChildProcess,
    ) -> Result<String, Box<dyn Error>> {
        let mut required = vec![proxy];
        if status == "Online" {
            required.push(diamant);
        }
        let host = client
            .wait_for_host(name, status, ACTION_TIMEOUT, &mut required)
            .await?;
        Ok(host["name"].as_str().ok_or("host name missing")?.to_owned())
    }

    /// Wait for every desired node to reflect the expected connection state.
    async fn wait_for_nodes(
        client: &ApiClient,
        total: usize,
        connected: bool,
        proxy: &mut ChildProcess,
        diamant: &mut ChildProcess,
    ) -> Result<(), Box<dyn Error>> {
        client
            .wait_for("/api/v1/nodes", ACTION_TIMEOUT, |nodes| {
                proxy.ensure_running()?;
                if connected {
                    diamant.ensure_running()?;
                }
                Ok(nodes["items"].as_array().is_some_and(|items| {
                    items.len() == total
                        && items.iter().all(|node| {
                            node["status"]["phase"]
                                == if connected { "Running" } else { "Disconnected" }
                        })
                }))
            })
            .await?;
        Ok(())
    }

    /// Wait for one node's desired name and runtime observation.
    async fn wait_for_node(
        client: &ApiClient,
        id: &str,
        name: &str,
        connected: bool,
    ) -> Result<(), Box<dyn Error>> {
        client
            .wait_for(&format!("/api/v1/nodes/{id}"), ACTION_TIMEOUT, |node| {
                Ok(node["displayName"] == name
                    && (!connected || node["status"]["phase"] == "Running"))
            })
            .await?;
        Ok(())
    }

    async fn wait_for_starts(path: &Path, script: &str, count: usize) -> io::Result<()> {
        let deadline = Instant::now() + ACTION_TIMEOUT;
        loop {
            let contents = fs::read_to_string(path).unwrap_or_default();
            let actual = contents
                .lines()
                .filter(|line| line.starts_with("START\t") && line.contains(script))
                .count();
            if actual >= count {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for {count} starts of {script}; saw {actual}"),
                ));
            }
            sleep(POLL_INTERVAL).await;
        }
    }

    async fn wait_for_all_processes_to_stop(path: &Path) -> io::Result<()> {
        let deadline = Instant::now() + ACTION_TIMEOUT;
        loop {
            let contents = fs::read_to_string(path).unwrap_or_default();
            let started: BTreeSet<_> = contents
                .lines()
                .filter_map(|line| line.strip_prefix("START\t"))
                .filter_map(|line| line.split_once('\t').map(|(pid, _)| pid))
                .collect();
            let stopped: BTreeSet<_> = contents
                .lines()
                .filter_map(|line| line.strip_prefix("STOP\t"))
                .collect();
            if !started.is_empty() && started == stopped {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "node processes did not all stop; started={started:?}, stopped={stopped:?}"
                    ),
                ));
            }
            sleep(POLL_INTERVAL).await;
        }
    }
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    unix::run().await
}

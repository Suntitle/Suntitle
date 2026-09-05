#[cfg(not(unix))]
fn main() {
    eprintln!("node_handling_e2e currently requires a Unix host");
}

#[cfg(unix)]
mod unix {
    use std::collections::BTreeSet;
    use std::env;
    use std::error::Error;
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::time::{Instant, sleep, timeout};
    use uuid::Uuid;

    const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
    const ACTION_TIMEOUT: Duration = Duration::from_secs(10);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    struct ChildProcess {
        name: &'static str,
        child: Child,
    }

    impl ChildProcess {
        fn spawn(name: &'static str, command: &mut Command) -> io::Result<Self> {
            let child = command
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()?;
            Ok(Self { name, child })
        }

        fn ensure_running(&mut self) -> io::Result<()> {
            if let Some(status) = self.child.try_wait()? {
                return Err(io::Error::other(format!(
                    "{} exited before the E2E check completed: {status}",
                    self.name
                )));
            }
            Ok(())
        }

        fn terminate(&self) -> io::Result<()> {
            let pid = i32::try_from(self.child.id())
                .map_err(|_| io::Error::other("child process ID exceeds i32"))?;
            kill(Pid::from_raw(pid), Signal::SIGTERM).map_err(io::Error::other)
        }

        async fn wait_for_exit(&mut self) -> io::Result<()> {
            let deadline = Instant::now() + ACTION_TIMEOUT;
            loop {
                if let Some(status) = self.child.try_wait()? {
                    if status.success() {
                        return Ok(());
                    }
                    return Err(io::Error::other(format!(
                        "{} exited unsuccessfully: {status}",
                        self.name
                    )));
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out waiting for {} to stop", self.name),
                    ));
                }
                sleep(POLL_INTERVAL).await;
            }
        }
    }

    impl Drop for ChildProcess {
        fn drop(&mut self) {
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
    }

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new() -> io::Result<Self> {
            let path = env::temp_dir().join(format!("suntitle-nodes-e2e-{}", Uuid::new_v4()));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub async fn run() -> Result<(), Box<dyn Error>> {
        build_helpers()?;
        let binary_directory = binary_directory()?;
        let temporary_directory = TemporaryDirectory::new()?;
        let identity_file = temporary_directory.0.join("diamant.id");
        let process_log = temporary_directory.0.join("node-processes.log");
        let host_name = format!("node-e2e-{}", Uuid::new_v4().simple());

        let mut proxy = ChildProcess::spawn(
            "ws-proxy",
            &mut Command::new(binary_path(&binary_directory, "ws-proxy")),
        )?;
        wait_for_proxy(&mut proxy).await?;

        let mut diamant_command = Command::new(binary_path(&binary_directory, "diamant"));
        diamant_command
            .arg("--proxy-url")
            .arg("ws://127.0.0.1:3030/register")
            .arg("--host-name")
            .arg(&host_name)
            .arg("--host-ip")
            .arg("192.0.2.20")
            .arg("--id-file")
            .arg(&identity_file)
            .arg("--health-interval-seconds")
            .arg("1")
            .arg("--shutdown-grace-seconds")
            .arg("2")
            .arg("--project-dir")
            .arg(project_directory())
            .arg("--uv-bin")
            .arg(binary_directory.join("examples").join("fake_uv"))
            .env("FAKE_NODE_LOG", &process_log);
        let mut diamant = ChildProcess::spawn("diamant", &mut diamant_command)?;

        let host_id = wait_for_host(&host_name, "online", &mut proxy, &mut diamant).await?;
        let audio = request_json(
            "POST",
            &format!("/api/v1/hosts/{host_id}/nodes"),
            Some(&audio_body("Studio microphone", 16_000)),
            201,
        )
        .await?;
        let audio_id = value_uuid(&audio, "id")?;
        let display = request_json(
            "POST",
            &format!("/api/v1/hosts/{host_id}/nodes"),
            Some(&display_body("Main display", 3)),
            201,
        )
        .await?;
        let display_id = value_uuid(&display, "id")?;

        wait_for_nodes(2, true, &mut proxy, &mut diamant).await?;
        wait_for_starts(&process_log, "suntitle-listener.py", 1).await?;
        wait_for_starts(&process_log, "suntitle.py", 1).await?;

        request_json(
            "POST",
            &format!("/api/v1/nodes/{display_id}/restart"),
            None,
            200,
        )
        .await?;
        wait_for_starts(&process_log, "suntitle.py", 2).await?;
        wait_for_node(&display_id, "Main display", true).await?;

        let update = json!({
            "type": "audio_source",
            "name": "Updated microphone",
            "description": "updated by node lifecycle E2E",
            "enabled": true,
            "settings": {
                "sampleRateHz": 48000,
                "channels": 1,
                "encoding": "pcm_s16le",
                "deviceName": null
            },
            "inputNodeId": null
        });
        request_json(
            "PUT",
            &format!("/api/v1/nodes/{audio_id}"),
            Some(&update),
            200,
        )
        .await?;
        wait_for_starts(&process_log, "suntitle-listener.py", 2).await?;
        wait_for_node(&audio_id, "Updated microphone", true).await?;

        request_json(
            "POST",
            &format!("/api/v1/hosts/{host_id}/nodes"),
            Some(&display_body("Auxiliary display", 5)),
            201,
        )
        .await?;
        wait_for_nodes(3, true, &mut proxy, &mut diamant).await?;
        wait_for_starts(&process_log, "suntitle.py", 3).await?;

        diamant.terminate()?;
        diamant.wait_for_exit().await?;
        wait_for_nodes(0, false, &mut proxy, &mut diamant).await?;
        wait_for_host(&host_name, "offline", &mut proxy, &mut diamant).await?;
        wait_for_all_processes_to_stop(&process_log).await?;

        println!(
            "PASS: node create/list/restart/update/add and Diamant shutdown cleanup completed for host {host_id}"
        );
        Ok(())
    }

    fn audio_body(name: &str, sample_rate: u32) -> Value {
        json!({
            "type": "audio_source",
            "name": name,
            "description": null,
            "enabled": true,
            "settings": {
                "sampleRateHz": sample_rate,
                "channels": 1,
                "encoding": "pcm_s16le",
                "deviceName": null
            }
        })
    }

    fn display_body(name: &str, max_lines: u8) -> Value {
        json!({
            "type": "display",
            "name": name,
            "description": null,
            "enabled": true,
            "settings": {
                "maxLines": max_lines,
                "fontScale": 1.0,
                "showOriginal": false,
                "theme": "dark"
            }
        })
    }

    fn value_uuid(value: &Value, key: &str) -> Result<Uuid, Box<dyn Error>> {
        Ok(Uuid::parse_str(value[key].as_str().ok_or_else(|| {
            io::Error::other(format!("response has no string {key}"))
        })?)?)
    }

    fn build_helpers() -> io::Result<()> {
        let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
        let status = Command::new(cargo)
            .arg("build")
            .arg("--manifest-path")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .arg("--bins")
            .arg("--example")
            .arg("fake_uv")
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "failed to build E2E binaries: {status}"
            )))
        }
    }

    fn binary_directory() -> io::Result<PathBuf> {
        env::current_exe()?
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| io::Error::other("could not determine Cargo target directory"))
    }

    fn binary_path(directory: &Path, name: &str) -> PathBuf {
        directory.join(format!("{name}{}", env::consts::EXE_SUFFIX))
    }

    fn project_directory() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("ws-proxy has a repository parent")
    }

    async fn wait_for_proxy(proxy: &mut ChildProcess) -> io::Result<()> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            proxy.ensure_running()?;
            if TcpStream::connect("127.0.0.1:3030").await.is_ok()
                && TcpStream::connect("127.0.0.1:3031").await.is_ok()
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for ws-proxy ports 3030 and 3031",
                ));
            }
            sleep(POLL_INTERVAL).await;
        }
    }

    async fn wait_for_host(
        name: &str,
        status: &str,
        proxy: &mut ChildProcess,
        diamant: &mut ChildProcess,
    ) -> Result<Uuid, Box<dyn Error>> {
        let deadline = Instant::now() + ACTION_TIMEOUT;
        loop {
            proxy.ensure_running()?;
            if status == "online" {
                diamant.ensure_running()?;
            }
            if let Ok(hosts) = request_json("GET", "/api/v1/hosts", None, 200).await
                && let Some(host) = hosts["items"].as_array().and_then(|items| {
                    items.iter().find(|host| {
                        host["name"] == name && host["status"].as_str() == Some(status)
                    })
                })
            {
                return value_uuid(host, "id");
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for host {name} to become {status}"),
                )
                .into());
            }
            sleep(POLL_INTERVAL).await;
        }
    }

    async fn wait_for_nodes(
        total: usize,
        connected: bool,
        proxy: &mut ChildProcess,
        diamant: &mut ChildProcess,
    ) -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + ACTION_TIMEOUT;
        loop {
            proxy.ensure_running()?;
            if total > 0 {
                diamant.ensure_running()?;
            }
            if let Ok(nodes) = request_json("GET", "/api/v1/nodes", None, 200).await
                && nodes["total"].as_u64() == Some(total as u64)
                && (!connected
                    || nodes["items"].as_array().is_some_and(|items| {
                        items.iter().all(|node| node["status"] == "connected")
                    }))
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for {total} API nodes"),
                )
                .into());
            }
            sleep(POLL_INTERVAL).await;
        }
    }

    async fn wait_for_node(id: &Uuid, name: &str, connected: bool) -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + ACTION_TIMEOUT;
        loop {
            if let Ok(node) = request_json("GET", &format!("/api/v1/nodes/{id}"), None, 200).await
                && node["name"] == name
                && (!connected || node["status"] == "connected")
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for node {id} to be updated"),
                )
                .into());
            }
            sleep(POLL_INTERVAL).await;
        }
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

    async fn request_json(
        method: &str,
        path: &str,
        body: Option<&Value>,
        expected_status: u16,
    ) -> Result<Value, Box<dyn Error>> {
        let body = body
            .map(serde_json::to_vec)
            .transpose()?
            .unwrap_or_default();
        let mut stream = TcpStream::connect("127.0.0.1:3031").await?;
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:3031\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(request.as_bytes()).await?;
        stream.write_all(&body).await?;

        let mut response = Vec::new();
        timeout(Duration::from_secs(2), stream.read_to_end(&mut response)).await??;
        let body_offset = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|offset| offset + 4)
            .ok_or_else(|| io::Error::other("control API returned an invalid HTTP response"))?;
        let headers = std::str::from_utf8(&response[..body_offset])?;
        let status = headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| io::Error::other("control API returned an invalid status line"))?;
        if status != expected_status {
            return Err(io::Error::other(format!(
                "{method} {path} returned {status}, expected {expected_status}: {}",
                String::from_utf8_lossy(&response[body_offset..])
            ))
            .into());
        }
        if response.len() == body_offset {
            Ok(Value::Null)
        } else {
            Ok(serde_json::from_slice(&response[body_offset..])?)
        }
    }
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    unix::run().await
}

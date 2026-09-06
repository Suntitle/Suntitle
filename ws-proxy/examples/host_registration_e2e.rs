mod support;

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;
use ws_proxy::api::models::{HostList, HostStatus};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(10);
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
        let path = env::temp_dir().join(format!("suntitle-host-e2e-{}", Uuid::new_v4()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    support::require_free_ports()?;
    let database = support::Database::create().await?;
    build_binaries()?;
    let binary_directory = binary_directory()?;
    let temporary_directory = TemporaryDirectory::new()?;
    let identity_file = temporary_directory.0.join("diamant.id");
    let host_name = format!("e2e-host-{}", Uuid::new_v4().simple());

    let _api = ChildProcess::spawn(
        "control_api",
        Command::new(binary_path(&binary_directory, "control_api"))
            .env("DATABASE_URL", &database.url),
    )?;
    let mut proxy = ChildProcess::spawn(
        "ws-proxy",
        Command::new(binary_path(&binary_directory, "ws-proxy")).env("DATABASE_URL", &database.url),
    )?;
    wait_for_proxy(&mut proxy).await?;

    let mut diamant_command = Command::new(binary_path(&binary_directory, "diamant"));
    diamant_command
        .arg("--proxy-url")
        .arg("ws://127.0.0.1:3030/register")
        .arg("--host-name")
        .arg(&host_name)
        .arg("--host-ip")
        .arg("192.0.2.10")
        .arg("--id-file")
        .arg(&identity_file)
        .arg("--health-interval-seconds")
        .arg("1");
    let mut diamant = ChildProcess::spawn("diamant", &mut diamant_command)?;

    let host = wait_for_registered_host(&host_name, &mut proxy, &mut diamant).await?;
    let expected_id = Uuid::parse_str(fs::read_to_string(&identity_file)?.trim())?;
    assert_eq!(host.id, expected_id);
    assert_eq!(host.name, host_name);
    assert_eq!(host.ip.to_string(), "192.0.2.10");
    assert_eq!(host.status, HostStatus::Online);
    assert!(
        host.metadata.as_ref().is_some_and(|metadata| {
            metadata.contains_key("os") && metadata.contains_key("arch")
        })
    );

    println!(
        "PASS: Diamant registered host {} and GET /api/v1/hosts returned it online",
        host.id
    );
    Ok(())
}

fn build_binaries() -> io::Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let status = Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .arg("--bins")
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "failed to build ws-proxy binaries: {status}"
        )));
    }
    Ok(())
}

fn binary_directory() -> io::Result<PathBuf> {
    let executable = env::current_exe()?;
    executable
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("could not determine Cargo target directory"))
}

fn binary_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}{}", env::consts::EXE_SUFFIX))
}

async fn wait_for_proxy(proxy: &mut ChildProcess) -> io::Result<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        proxy.ensure_running()?;
        let websocket_ready = TcpStream::connect("127.0.0.1:3030").await.is_ok();
        let api_ready = TcpStream::connect("127.0.0.1:3031").await.is_ok();
        if websocket_ready && api_ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for proxy port 3030 and API port 3031",
            ));
        }
        sleep(POLL_INTERVAL).await;
    }
}

async fn wait_for_registered_host(
    host_name: &str,
    proxy: &mut ChildProcess,
    diamant: &mut ChildProcess,
) -> Result<ws_proxy::api::models::Host, Box<dyn Error>> {
    let deadline = Instant::now() + REGISTRATION_TIMEOUT;
    loop {
        proxy.ensure_running()?;
        diamant.ensure_running()?;
        if let Ok(hosts) = list_hosts().await
            && let Some(host) = hosts.items.into_iter().find(|host| host.name == host_name)
        {
            return Ok(host);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out waiting for registered host {host_name}"),
            )
            .into());
        }
        sleep(POLL_INTERVAL).await;
    }
}

async fn list_hosts() -> Result<HostList, Box<dyn Error>> {
    let mut stream = TcpStream::connect("127.0.0.1:3031").await?;
    stream
        .write_all(
            b"GET /api/v1/hosts HTTP/1.1\r\nHost: 127.0.0.1:3031\r\nConnection: close\r\n\r\n",
        )
        .await?;

    let mut response = Vec::new();
    timeout(Duration::from_secs(2), stream.read_to_end(&mut response)).await??;
    let body_offset = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|offset| offset + 4)
        .ok_or_else(|| io::Error::other("control API returned an invalid HTTP response"))?;
    let headers = std::str::from_utf8(&response[..body_offset])?;
    if !headers.starts_with("HTTP/1.1 200") {
        return Err(io::Error::other(format!(
            "control API returned a non-200 response: {}",
            headers.lines().next().unwrap_or("unknown status")
        ))
        .into());
    }

    Ok(serde_json::from_slice(&response[body_offset..])?)
}

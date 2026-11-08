use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Args, Parser, Subcommand, builder::NonEmptyStringValueParser};
use diamant::artifacts::{ArtifactDownloads, ArtifactOptions};
use diamant::nodes::{
    NodeEvent, NodeProcessManager, NodeRuntimeStatus, data_plane_url_from_registration,
};
#[cfg(feature = "gui")]
use diamant_ui::{AgentConfig, Reporter};
use futures_util::{SinkExt, StreamExt};
use rootcause::compat::MainReport;
use rootcause::option_ext::OptionExt;
use rootcause::prelude::*;
use suntitle_core::registration::{
    Envelope, Operation, RegistrationClientMessage, RegistrationServerMessage,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Error as WebSocketError, Message},
};
use url::Url;

const DEFAULT_PROXY_URL: &str = "ws://127.0.0.1:3030/register";
const DEFAULT_HEALTH_INTERVAL_SECONDS: u64 = 30;
const DEFAULT_SHUTDOWN_GRACE_SECONDS: u64 = 5;

/// Runs the Diamant host agent.
#[derive(Clone, Debug, Parser)]
#[command(version, about)]
#[cfg_attr(
    feature = "gui",
    command(about = "Opens the Diamant desktop UI or runs its headless host agent.")
)]
struct Config {
    /// Run headless. This is the default in builds without the gui feature.
    #[arg(long)]
    cli: bool,

    /// WebSocket registration endpoint exposed by VS Proxy.
    #[arg(
        long,
        env = "PROXY_URL",
        default_value = DEFAULT_PROXY_URL,
        value_parser = NonEmptyStringValueParser::new()
    )]
    proxy_url: String,

    /// Name used to identify this host in Suntitle.
    #[arg(
        long,
        env = "HOST_NAME",
        default_value_t = default_host_name(),
        value_parser = NonEmptyStringValueParser::new()
    )]
    host_name: String,

    /// Host IP reported to VS Proxy. By default, Diamant detects the routed IPv4 address.
    #[arg(long, env = "HOST_IP")]
    host_ip: Option<IpAddr>,

    /// Seconds between health OK reports.
    #[arg(
        long,
        env = "HEALTH_INTERVAL_SECONDS",
        default_value_t = DEFAULT_HEALTH_INTERVAL_SECONDS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    health_interval_seconds: u64,

    /// Persistent directory for downloaded extension artifacts. Defaults to OS local data storage.
    #[arg(long, env = "ARTIFACT_DIR")]
    artifact_dir: Option<PathBuf>,

    /// Use plain HTTP for artifact registry downloads.
    #[arg(long, env = "ARTIFACT_PLAIN_HTTP")]
    artifact_plain_http: bool,

    /// WebSocket data-plane base URL. Defaults to the registration URL without /register.
    #[arg(long, env = "DATA_PLANE_URL")]
    data_plane_url: Option<Url>,

    /// Seconds allowed for children to exit after SIGTERM.
    #[arg(long, env = "SHUTDOWN_GRACE_SECONDS", default_value_t = DEFAULT_SHUTDOWN_GRACE_SECONDS, value_parser = clap::value_parser!(u64).range(1..))]
    shutdown_grace_seconds: u64,

    #[command(subcommand)]
    command: Option<Command>,
}

#[cfg(feature = "gui")]
impl Config {
    fn agent_config(&self) -> AgentConfig {
        AgentConfig {
            proxy_url: self.proxy_url.clone(),
            host_name: self.host_name.clone(),
            host_ip: self.host_ip,
            health_interval_seconds: self.health_interval_seconds,
            data_plane_url: self.data_plane_url.clone(),
            shutdown_grace_seconds: self.shutdown_grace_seconds,
        }
    }
}

#[derive(Clone, Subcommand, Debug)]
enum Command {
    /// Invoke the embedded uv CLI.
    Uv(UvArgs),
}

#[derive(Args, Clone, Debug)]
struct UvArgs {
    /// Everything after `uv` is forwarded verbatim.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        num_args = 0..
    )]
    args: Vec<OsString>,
}

fn default_host_name() -> String {
    hostname::get()
        .ok()
        .and_then(|name| name.into_string().ok())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "unknown-host".to_owned())
}

#[tokio::main]
async fn main() -> Result<ExitCode, MainReport> {
    let config = Config::parse();
    if let Some(Command::Uv(UvArgs { args })) = &config.command {
        let args = std::iter::once(std::ffi::OsString::from("uv")).chain(args.clone());
        return Ok(unsafe { uv::main(args) });
    }
    #[cfg(feature = "gui")]
    if !config.cli {
        let artifacts = ArtifactOptions {
            directory: config.artifact_dir.clone(),
            plain_http: config.artifact_plain_http,
        };
        return Ok(diamant_ui::run(
            config.agent_config(),
            move |config, stop, reporter| run_gui_agent(config, stop, reporter, artifacts.clone()),
        ));
    }

    let _agent_lock = lock_host(&config.host_name)
        .context("another Diamant agent is already running for this host")?;
    pretty_env_logger::init();
    run_agent(
        config,
        None,
        #[cfg(feature = "gui")]
        None,
    )
    .await;
    Ok(ExitCode::default())
}

#[cfg(feature = "gui")]
fn run_gui_agent(
    config: AgentConfig,
    stop: Arc<AtomicBool>,
    reporter: Reporter,
    artifacts: ArtifactOptions,
) {
    let config = Config {
        cli: true,
        artifact_dir: artifacts.directory,
        artifact_plain_http: artifacts.plain_http,
        proxy_url: config.proxy_url,
        host_name: config.host_name,
        host_ip: config.host_ip,
        health_interval_seconds: config.health_interval_seconds,
        data_plane_url: config.data_plane_url,
        shutdown_grace_seconds: config.shutdown_grace_seconds,
        command: None,
    };
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(run_agent(config, Some(stop), Some(reporter))),
        Err(error) => reporter.failed(&format!("Failed to start runtime: {error}")),
    }
}

async fn run_agent(
    config: Config,
    stop: Option<Arc<AtomicBool>>,
    #[cfg(feature = "gui")] reporter: Option<Reporter>,
) {
    loop {
        if is_stopped(&stop) {
            break;
        }
        #[cfg(feature = "gui")]
        if let Some(reporter) = &reporter {
            reporter.connection("connecting", "");
        }
        match register(
            &config,
            stop.clone(),
            #[cfg(feature = "gui")]
            reporter.as_ref(),
        )
        .await
        {
            Ok(false) => break,
            Ok(true) => {}
            Err(error) => {
                eprintln!("registration disconnected: {error}");
                #[cfg(feature = "gui")]
                if let Some(reporter) = &reporter {
                    reporter.connection("error", &error.to_string());
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {},
            _ = shutdown_requested(stop.clone()) => break,
        }
    }
    #[cfg(feature = "gui")]
    if let Some(reporter) = &reporter {
        reporter.finished();
    }
}

fn is_stopped(stop: &Option<Arc<AtomicBool>>) -> bool {
    stop.as_ref()
        .is_some_and(|stop| stop.load(Ordering::Relaxed))
}

async fn shutdown_requested(stop: Option<Arc<AtomicBool>>) {
    match stop {
        Some(stop) => {
            while !stop.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        None => shutdown_signal().await,
    }
}

fn connection_ip(stream: &MaybeTlsStream<tokio::net::TcpStream>) -> rootcause::Result<IpAddr> {
    match stream {
        MaybeTlsStream::Plain(stream) => Ok(stream
            .local_addr()
            .context("failed to determine the registration connection's local address")?
            .ip()),
        _ => rootcause::bail!(
            "automatic IP detection is unavailable for this transport; use --host-ip"
        ),
    }
}

fn host_metadata() -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        (
            "os".to_owned(),
            serde_json::Value::String(std::env::consts::OS.to_owned()),
        ),
        (
            "arch".to_owned(),
            serde_json::Value::String(std::env::consts::ARCH.to_owned()),
        ),
    ])
}

async fn register(
    config: &Config,
    stop: Option<Arc<AtomicBool>>,
    #[cfg(feature = "gui")] reporter: Option<&Reporter>,
) -> rootcause::Result<bool> {
    let registration_url = Url::parse(&config.proxy_url).context("invalid proxy URL")?;
    let data_plane_url = match &config.data_plane_url {
        Some(url) => url.clone(),
        None => data_plane_url_from_registration(&registration_url).map_err(|message| {
            rootcause::report!(io::Error::new(io::ErrorKind::InvalidInput, message))
        })?,
    };
    let mut manager = NodeProcessManager::new(
        std::env::current_exe().context("unknown path for diamant command")?,
        data_plane_url,
        Duration::from_secs(config.shutdown_grace_seconds),
    )
    .map_err(|message| rootcause::report!(io::Error::new(io::ErrorKind::InvalidInput, message)))?;
    let (socket, _) = connect_async(registration_url.as_str())
        .await
        .context_with(|| format!("failed to connect to {}", config.proxy_url))?;
    let host_ip = match config.host_ip {
        Some(ip) => ip,
        None => connection_ip(socket.get_ref())?,
    };
    let (mut write, mut read) = socket.split();
    let metadata = host_metadata();
    let request = Envelope::new(RegistrationClientMessage::Register {
        host_name: config.host_name.clone(),
        agent_version: env!("CARGO_PKG_VERSION").into(),
        ip: Some(host_ip.to_string()),
        reported_metadata: metadata,
    });
    write
        .send(Message::Text(
            serde_json::to_string(&request)
                .context("failed to serialize host registration")?
                .into(),
        ))
        .await
        .context("failed to send host registration")?;

    let response = read
        .next()
        .await
        .context("proxy closed before acknowledging registration")?
        .context("failed to receive registration acknowledgment")?;
    let response: Envelope<RegistrationServerMessage> =
        serde_json::from_slice(&response.into_data())
            .context("proxy returned an invalid registration acknowledgment")?;
    if response.protocol_version != 1 {
        rootcause::bail!("unsupported protocol version");
    }
    let (host, session_id) = match response.message {
        RegistrationServerMessage::Registered { host, session_id } => {
            println!(
                "registered host {}/{} ({})",
                host.namespace, host.name, host.uid
            );
            (host, session_id)
        }
        RegistrationServerMessage::Error { message } => {
            rootcause::bail!("proxy rejected host registration: {message}")
        }
        _ => rootcause::bail!("proxy sent a command before registration"),
    };
    #[cfg(feature = "gui")]
    if let Some(reporter) = reporter {
        reporter.connection("connected", &format!("Registered as {}", host.name));
        reporter.nodes(&manager.snapshots());
    }
    let health = RegistrationClientMessage::Heartbeat {
        host_uid: host.uid.clone(),
        session_id: session_id.clone(),
    };
    let mut health_interval =
        tokio::time::interval(Duration::from_secs(config.health_interval_seconds));
    health_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    health_interval.tick().await;
    let mut reap_interval = tokio::time::interval(Duration::from_millis(250));
    reap_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    reap_interval.tick().await;
    let shutdown = shutdown_requested(stop);
    tokio::pin!(shutdown);
    let mut artifacts = ArtifactDownloads::new(ArtifactOptions {
        directory: config.artifact_dir.clone(),
        plain_http: config.artifact_plain_http,
    })
    .context("failed to configure artifact downloads")?;
    let mut failure = None;
    let mut reconnect = true;

    loop {
        tokio::select! {
            report = artifacts.next_report() => {
                if let RegistrationClientMessage::Acknowledgement { identity, success: true, operation: Operation::Download, .. } = &report
                    && let Some(installation) = artifacts.take_prepared(identity) {
                    manager.prepare_extension(installation);
                }
                if let Err(error) = send_client_message(&mut write, &report).await {
                    failure = Some(format!("failed to report artifact download: {error}"));
                    break;
                }
            }
            _ = health_interval.tick() => {
                if let Err(error) = send_client_message(&mut write, &health).await {
                    failure = Some(format!("failed to send health report: {error}"));
                    break;
                }
            }
            _ = reap_interval.tick() => {
                let events = manager.reap_exited();
                #[cfg(feature = "gui")]
                let changed = !events.is_empty();
                if let Err(error) = send_events(&mut write, events).await {
                    failure = Some(format!("failed to report node exit: {error}"));
                    break;
                }
                #[cfg(feature = "gui")]
                if changed {
                    if let Some(reporter) = reporter {
                        reporter.nodes(&manager.snapshots());
                    }
                }
            }
            _ = &mut shutdown => { reconnect = false; break; },
            incoming = read.next() => {
                match incoming {
                    Some(Ok(message)) if message.is_close() => break,
                    Some(Ok(message)) if message.is_ping() => {
                        if let Err(error) = write.send(Message::Pong(message.into_data())).await {
                            failure = Some(format!("failed to answer proxy ping: {error}"));
                            break;
                        }
                    }
                    Some(Ok(message)) if message.is_text() || message.is_binary() => {
                        let command = match serde_json::from_slice::<Envelope<RegistrationServerMessage>>(&message.into_data()) {
                            Ok(command) => command,
                            Err(error) => {
                                failure = Some(format!("proxy sent an invalid node command: {error}"));
                                break;
                            }
                        };
                        if command.protocol_version != 1 { break; }
                        let events = match command.message {
                            RegistrationServerMessage::Apply { identity, spec, custom } => {
                                if identity.host_uid != host.uid || identity.session_id != session_id || identity.target.namespace != host.namespace || spec.host_ref.name != host.name || identity.restart_token != spec.restart_token { break; }
                                manager.apply(identity, *spec, custom.map(|custom| *custom)).await
                            },
                            RegistrationServerMessage::DownloadArtifact { identity, reference, adapter } => {
                                if identity.host_uid != host.uid || identity.session_id != session_id || identity.target.namespace != host.namespace { break; }
                                artifacts.start(identity, reference, *adapter);
                                Vec::new()
                            },
                            RegistrationServerMessage::Remove { identity } => {
                                if identity.host_uid != host.uid || identity.session_id != session_id || identity.target.namespace != host.namespace { break; }
                                artifacts.remove(&identity).await;
                                let result = manager.remove(&identity).await;
                                let acknowledgement = RegistrationClientMessage::Acknowledgement {
                                    identity, operation: Operation::Remove, success: result.is_ok(), message: result.err(),
                                };
                                if let Err(error) = send_client_message(&mut write, &acknowledgement).await {
                                    failure = Some(format!("failed to report cleanup: {error}"));
                                    break;
                                }
                                Vec::new()
                            },
                            RegistrationServerMessage::Error { message } => {
                                failure = Some(format!("proxy error: {message}"));
                                break;
                            }
                            RegistrationServerMessage::Registered { .. } => Vec::new(),
                        };
                        if let Err(error) = send_events(&mut write, events).await {
                            failure = Some(format!("failed to report node lifecycle: {error}"));
                            break;
                        }
                        #[cfg(feature = "gui")]
                        if let Some(reporter) = reporter {
                            reporter.nodes(&manager.snapshots());
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        failure = Some(format!("failed to receive a proxy message: {error}"));
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    artifacts.shutdown().await;
    let _ = send_events(&mut write, manager.shutdown_all().await).await;
    #[cfg(feature = "gui")]
    if let Some(reporter) = reporter {
        reporter.nodes(&manager.snapshots());
    }
    let _ = write.send(Message::Close(None)).await;
    if let Some(message) = failure {
        rootcause::bail!(message);
    }
    Ok(reconnect)
}

type RegistrationWriter = futures_util::stream::SplitSink<
    WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

async fn send_client_message(
    write: &mut RegistrationWriter,
    message: &RegistrationClientMessage,
) -> Result<(), Box<WebSocketError>> {
    let json = serde_json::to_string(&Envelope::new(message))
        .map_err(|error| Box::new(WebSocketError::Io(io::Error::other(error))))?;
    write
        .send(Message::Text(json.into()))
        .await
        .map_err(Box::new)
}

async fn send_events(
    write: &mut RegistrationWriter,
    events: Vec<NodeEvent>,
) -> Result<(), Box<WebSocketError>> {
    for event in events {
        let message = RegistrationClientMessage::Acknowledgement {
            identity: event.identity,
            operation: Operation::Apply,
            success: event.status != NodeRuntimeStatus::Failed,
            message: event.message,
        };
        send_client_message(write, &message).await?;
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

fn lock_host(host_name: &str) -> io::Result<std::fs::File> {
    use std::hash::{Hash, Hasher};
    let mut key = std::collections::hash_map::DefaultHasher::new();
    host_name.hash(&mut key);
    let path = std::env::temp_dir().join(format!("suntitle-diamant-{:x}.lock", key.finish()));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.try_lock().map_err(io::Error::other)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_lock_rejects_duplicates_and_releases_on_exit() {
        let host = uuid::Uuid::new_v4().to_string();
        let first = lock_host(&host).unwrap();
        assert!(lock_host(&host).is_err());
        assert!(lock_host(&format!("{host}-other")).is_ok());
        drop(first);
        assert!(lock_host(&host).is_ok());
    }
}

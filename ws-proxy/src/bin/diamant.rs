use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Parser, builder::NonEmptyStringValueParser};
use futures_util::{SinkExt, StreamExt};
use rootcause::compat::MainReport;
use rootcause::option_ext::OptionExt;
use rootcause::prelude::*;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Error as WebSocketError, Message},
};
use url::Url;
use uuid::Uuid;
use ws_proxy::diamant::nodes::{NodeEvent, NodeProcessManager, data_plane_url_from_registration};
use ws_proxy::registration::{RegistrationClientMessage, RegistrationServerMessage};

const DEFAULT_PROXY_URL: &str = "ws://127.0.0.1:3030/register";
const DEFAULT_HEALTH_INTERVAL_SECONDS: u64 = 30;
const DEFAULT_ID_FILE: &str = "diamant.id";
const DEFAULT_SHUTDOWN_GRACE_SECONDS: u64 = 5;

/// Registers this host with a Suntitle VS Proxy.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Config {
    /// WebSocket registration endpoint exposed by VS Proxy.
    #[arg(
        long,
        env = "SUNTITLE_PROXY_URL",
        default_value = DEFAULT_PROXY_URL,
        value_parser = NonEmptyStringValueParser::new()
    )]
    proxy_url: String,

    /// Name used to identify this host in Suntitle.
    #[arg(
        long,
        env = "SUNTITLE_HOST_NAME",
        default_value_t = default_host_name(),
        value_parser = NonEmptyStringValueParser::new()
    )]
    host_name: String,

    /// Host IP reported to VS Proxy. By default, Diamant detects the routed IPv4 address.
    #[arg(long, env = "SUNTITLE_HOST_IP")]
    host_ip: Option<IpAddr>,

    /// File used to preserve this host's UUID across restarts.
    #[arg(
        long,
        env = "SUNTITLE_ID_FILE",
        default_value = DEFAULT_ID_FILE
    )]
    id_file: PathBuf,

    /// Seconds between health OK reports.
    #[arg(
        long,
        env = "SUNTITLE_HEALTH_INTERVAL_SECONDS",
        default_value_t = DEFAULT_HEALTH_INTERVAL_SECONDS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    health_interval_seconds: u64,

    /// Repository directory containing pyproject.toml and the Python entry points.
    #[arg(long, env = "SUNTITLE_PROJECT_DIR", default_value = ".")]
    project_dir: PathBuf,

    /// uv executable used to launch node processes.
    #[arg(long, env = "SUNTITLE_UV_BIN", default_value = "uv")]
    uv_bin: PathBuf,

    /// WebSocket data-plane base URL. Defaults to the registration URL without /register.
    #[arg(long, env = "SUNTITLE_DATA_PLANE_URL")]
    data_plane_url: Option<Url>,

    /// Seconds allowed for children to exit after SIGTERM.
    #[arg(long, env = "SUNTITLE_SHUTDOWN_GRACE_SECONDS", default_value_t = DEFAULT_SHUTDOWN_GRACE_SECONDS, value_parser = clap::value_parser!(u64).range(1..))]
    shutdown_grace_seconds: u64,
}

fn default_host_name() -> String {
    hostname::get()
        .ok()
        .and_then(|name| name.into_string().ok())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "unknown-host".to_owned())
}

#[tokio::main]
async fn main() -> Result<(), MainReport> {
    pretty_env_logger::init();
    let config = Config::parse();
    let host_id = load_or_create_host_id(&config.id_file)?;
    register(&config, host_id).await?;
    Ok(())
}

fn load_or_create_host_id(path: &Path) -> rootcause::Result<Uuid> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Uuid::parse_str(contents.trim())
            .context_with(|| format!("invalid host UUID in {}", path.display()))?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent).context_with(|| {
                    format!("failed to create identity directory {}", parent.display())
                })?;
            }

            let host_id = Uuid::new_v4();
            fs::write(path, format!("{host_id}\n"))
                .context_with(|| format!("failed to write host UUID to {}", path.display()))?;
            Ok(host_id)
        }
        Err(error) => Err(rootcause::report!(error)
            .context(format!("failed to read identity file {}", path.display()))
            .into_dynamic()),
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

async fn register(config: &Config, host_id: Uuid) -> rootcause::Result<()> {
    let registration_url = Url::parse(&config.proxy_url).context("invalid proxy URL")?;
    let data_plane_url = match &config.data_plane_url {
        Some(url) => url.clone(),
        None => data_plane_url_from_registration(&registration_url).map_err(|message| {
            rootcause::report!(io::Error::new(io::ErrorKind::InvalidInput, message))
        })?,
    };
    let mut manager = NodeProcessManager::new(
        config.project_dir.clone(),
        config.uv_bin.clone(),
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
    let metadata = Some(host_metadata());
    let request = RegistrationClientMessage::RegisterHost {
        id: host_id,
        name: config.host_name.clone(),
        ip: host_ip,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        metadata: metadata.clone(),
    };
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
    let response: RegistrationServerMessage = serde_json::from_slice(&response.into_data())
        .context("proxy returned an invalid registration acknowledgment")?;
    match response {
        RegistrationServerMessage::Registered { host } => {
            println!(
                "registered host {} as {} at {} (status: {:?})",
                host.name, host.id, host.ip, host.status
            );
        }
        RegistrationServerMessage::Error { message } => {
            rootcause::bail!("proxy rejected host registration: {message}")
        }
        _ => rootcause::bail!("proxy sent a node command before acknowledging registration"),
    }

    let health = RegistrationClientMessage::HealthOk {
        host_id,
        ip: host_ip,
        metadata,
    };
    let mut health_interval =
        tokio::time::interval(Duration::from_secs(config.health_interval_seconds));
    health_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    health_interval.tick().await;
    let mut reap_interval = tokio::time::interval(Duration::from_millis(250));
    reap_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    reap_interval.tick().await;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut failure = None;

    loop {
        tokio::select! {
            _ = health_interval.tick() => {
                if let Err(error) = send_client_message(&mut write, &health).await {
                    failure = Some(format!("failed to send health report: {error}"));
                    break;
                }
            }
            _ = reap_interval.tick() => {
                if let Err(error) = send_events(&mut write, host_id, manager.reap_exited()).await {
                    failure = Some(format!("failed to report node exit: {error}"));
                    break;
                }
            }
            _ = &mut shutdown => break,
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
                        let command = match serde_json::from_slice::<RegistrationServerMessage>(&message.into_data()) {
                            Ok(command) => command,
                            Err(error) => {
                                failure = Some(format!("proxy sent an invalid node command: {error}"));
                                break;
                            }
                        };
                        let events = match command {
                            RegistrationServerMessage::ApplyNode { node_id, revision, update } => manager.apply(node_id, revision, update).await,
                            RegistrationServerMessage::RestartNode { node_id, revision } => manager.restart(node_id, revision).await,
                            RegistrationServerMessage::Error { message } => {
                                failure = Some(format!("proxy error: {message}"));
                                break;
                            }
                            RegistrationServerMessage::Registered { .. } => Vec::new(),
                        };
                        if let Err(error) = send_events(&mut write, host_id, events).await {
                            failure = Some(format!("failed to report node lifecycle: {error}"));
                            break;
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

    let _ = send_events(&mut write, host_id, manager.shutdown_all().await).await;
    let _ = write.send(Message::Close(None)).await;
    if let Some(message) = failure {
        rootcause::bail!(message);
    }
    Ok(())
}

type RegistrationWriter = futures_util::stream::SplitSink<
    WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

async fn send_client_message(
    write: &mut RegistrationWriter,
    message: &RegistrationClientMessage,
) -> Result<(), Box<WebSocketError>> {
    let json = serde_json::to_string(message)
        .map_err(|error| Box::new(WebSocketError::Io(io::Error::other(error))))?;
    write
        .send(Message::Text(json.into()))
        .await
        .map_err(Box::new)
}

async fn send_events(
    write: &mut RegistrationWriter,
    host_id: Uuid,
    events: Vec<NodeEvent>,
) -> Result<(), Box<WebSocketError>> {
    for event in events {
        let message = RegistrationClientMessage::NodeStatus {
            host_id,
            node_id: event.node_id,
            revision: event.revision,
            status: event.status,
            message: event.message,
        };
        send_client_message(write, &message).await?;
    }
    Ok(())
}

async fn shutdown_signal() {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    ctrlc::set_handler(move || {
        let _ = sender.send(());
    })
    .expect("shutdown signal handler installs");
    let _ = receiver.recv().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_id_is_reused_from_the_identity_file() {
        let directory = std::env::temp_dir().join(format!("diamant-test-{}", Uuid::new_v4()));
        let identity_file = directory.join("host.id");

        let first = load_or_create_host_id(&identity_file).expect("create host identity");
        let second = load_or_create_host_id(&identity_file).expect("read host identity");

        assert_eq!(first, second);
        fs::remove_file(identity_file).expect("remove identity file");
        fs::remove_dir(directory).expect("remove test directory");
    }
}

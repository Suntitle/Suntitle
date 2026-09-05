use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Parser, builder::NonEmptyStringValueParser};
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use rootcause::compat::MainReport;
use rootcause::option_ext::OptionExt;
use rootcause::prelude::*;
use tokio_stream::wrappers::IntervalStream;
use tokio_tungstenite::{
    MaybeTlsStream, connect_async,
    tungstenite::{Error as WebSocketError, Message},
};
use uuid::Uuid;
use ws_proxy::registration::{RegistrationClientMessage, RegistrationServerMessage};

const DEFAULT_PROXY_URL: &str = "ws://127.0.0.1:3030/register";
const DEFAULT_HEALTH_INTERVAL_SECONDS: u64 = 30;
const DEFAULT_ID_FILE: &str = "diamant.id";

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
    let (socket, _) = connect_async(&config.proxy_url)
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
    }

    let health = RegistrationClientMessage::HealthOk {
        host_id,
        ip: host_ip,
        metadata,
    };
    let health_message = Message::Text(
        serde_json::to_string(&health)
            .context("failed to serialize health report")?
            .into(),
    );
    let mut health_interval =
        tokio::time::interval(Duration::from_secs(config.health_interval_seconds));
    health_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let health_reports = IntervalStream::new(health_interval)
        .skip(1)
        .map(move |_| Ok::<_, Box<WebSocketError>>(health_message.clone()))
        .forward(write.sink_map_err(Box::new));
    let proxy_messages = read.try_for_each(|_| async { Ok(()) });

    tokio::select! {
        result = health_reports => result.context("failed to send health report")?,
        result = proxy_messages => result.context("failed to receive a proxy message")?,
    }

    Ok(())
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

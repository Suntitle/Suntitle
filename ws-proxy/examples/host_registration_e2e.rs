//! Verify host registration through real API, proxy, and Diamant processes.
//! Recipes MUST prepare binaries before startup. Guards stop applications before namespace cleanup.
mod support;

use control_api::api::hosts::Host;
use std::{error::Error, io, process::Command, time::Duration};
use suntitle_core::kubernetes::resources::Presence;
use support::{ApiClient, ChildProcess, binary_directory, binary_path, wait_for_services};
use uuid::Uuid;

/// Maximum time for host registration to appear in the control API.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Run the host registration assertions using shared process and API helpers.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    support::require_free_ports()?;
    let binary_directory = binary_directory()?;
    let namespace = crate::support::TestNamespace::new()
        .await
        .map_err(io::Error::other)?;
    let host_name = format!("e2e-host-{}", Uuid::new_v4().simple());

    let mut api = ChildProcess::spawn(
        "control_api",
        &mut namespace.command(binary_path(&binary_directory, "control_api"), "api"),
    )?;
    let mut proxy = ChildProcess::spawn(
        "ws-proxy",
        &mut namespace.command(binary_path(&binary_directory, "ws-proxy"), "proxy"),
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
        .arg("192.0.2.10")
        .arg("--health-interval-seconds")
        .arg("1");
    let mut diamant = ChildProcess::spawn("diamant", &mut diamant_command)?;

    let client = ApiClient::new()?;
    let host: Host = serde_json::from_value(
        client
            .wait_for_host(
                &host_name,
                "Online",
                REGISTRATION_TIMEOUT,
                &mut [&mut proxy, &mut diamant],
            )
            .await?,
    )?;
    assert_eq!(host.name, host_name);
    assert_eq!(host.spec.display_name, host_name);
    let status = host.status.expect("host status");
    assert_eq!(status.ip.as_deref(), Some("192.0.2.10"));
    assert!(matches!(status.presence, Some(Presence::Online)));
    assert!(status.reported_metadata.contains_key("os"));
    assert!(status.reported_metadata.contains_key("arch"));

    println!("PASS: Diamant registered host {host_name} and GET /api/v1/hosts returned it online");
    Ok(())
}

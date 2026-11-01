use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use warp::Filter;
use ws_proxy::{
    diamant::artifacts::{ArtifactDownloads, ArtifactOptions},
    kubernetes::resources::ResourceIdentity,
    registration::*,
};

fn identity() -> ApplyIdentity {
    ApplyIdentity {
        target: ResourceIdentity {
            namespace: "suntitle".into(),
            name: "custom".into(),
            uid: "node".into(),
        },
        host_uid: "host".into(),
        session_id: "session".into(),
        generation: 1,
        restart_token: None,
        operation_id: "download".into(),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

async fn registry(corrupt: bool, delay: Duration) -> (String, tokio::task::JoinHandle<()>) {
    let mut archive = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(7);
    header.set_mode(0o751);
    header.set_cksum();
    archive
        .append_data(&mut header, "payload/run", &b"payload"[..])
        .unwrap();
    let tar = archive.into_inner().unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, &tar).unwrap();
    let blob = encoder.finish().unwrap();
    let manifest = json!({ "schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType":"application/vnd.oci.empty.v1+json", "digest":digest(b"{}"),"size":2},
        "layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar+gzip", "digest":digest(&blob), "size":blob.len(),
            "annotations":{"org.opencontainers.image.title":"payload","io.deis.oras.content.unpack":"true"}}] });
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let route = warp::path::full().and_then(move |path: warp::path::FullPath| {
        let manifest = manifest.clone();
        let blob = blob.clone();
        async move {
            let (content, media) = if path.as_str().contains("/manifests/") {
                (manifest, "application/vnd.oci.image.manifest.v1+json")
            } else if path.as_str().contains("/blobs/") {
                tokio::time::sleep(delay).await;
                (
                    if corrupt { b"corrupt".to_vec() } else { blob },
                    "application/octet-stream",
                )
            } else {
                (b"{}".to_vec(), "application/json")
            };
            Ok::<_, std::convert::Infallible>(
                warp::http::Response::builder()
                    .header("Content-Type", media)
                    .body(content)
                    .unwrap(),
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    (
        format!("{address}/extensions:fixture"),
        tokio::spawn(warp::serve(route).incoming(listener).run()),
    )
}

/// Reuse one client across transfers without publishing failed downloads.
#[tokio::test]
async fn downloads_publish_complete_trees_and_report_failures_without_partial_outputs() {
    let root = tempfile::tempdir().unwrap();
    let options = ArtifactOptions {
        directory: Some(root.path().into()),
        plain_http: true,
    };
    let mut downloads = ArtifactDownloads::new(options).unwrap();
    for corrupt in [false, true] {
        let (reference, server) = registry(corrupt, Duration::ZERO).await;
        let mut id = identity();
        id.generation += i64::from(corrupt);
        id.operation_id = format!("transfer-{}", id.generation);
        downloads.start(id.clone(), reference);
        let report = tokio::time::timeout(Duration::from_secs(10), downloads.next_report())
            .await
            .unwrap();
        let RegistrationClientMessage::Acknowledgement {
            identity: actual,
            operation,
            success,
            message,
        } = report
        else {
            panic!("expected report")
        };
        assert_eq!(actual, id);
        assert_eq!(operation, Operation::Download);
        assert_eq!(success, !corrupt, "{message:?}");
        server.abort();
        let entries = std::fs::read_dir(root.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
        let payload = entries[0].path().join("payload/run");
        assert_eq!(std::fs::read(&payload).unwrap(), b"payload");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&payload).unwrap().permissions().mode() & 0o777,
                0o751
            );
        }
    }
    downloads.shutdown().await;
}

/// Removal and shutdown MUST join cancelled work without reporting stale results.
#[tokio::test]
async fn replacement_and_shutdown_cancel_downloads_and_discard_stale_reports() {
    let root = tempfile::tempdir().unwrap();
    let mut downloads = ArtifactDownloads::new(ArtifactOptions {
        directory: Some(root.path().into()),
        plain_http: true,
    })
    .unwrap();
    let (reference, server) = registry(false, Duration::from_secs(30)).await;
    let id = identity();
    downloads.start(id.clone(), reference.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut next = id.clone();
    next.generation += 1;
    next.operation_id = "replacement".into();
    downloads.start(next.clone(), "invalid reference".into());
    downloads.start(id, reference.clone());
    let report = tokio::time::timeout(Duration::from_secs(5), downloads.next_report())
        .await
        .unwrap();
    let RegistrationClientMessage::Acknowledgement {
        identity, success, ..
    } = report
    else {
        panic!("expected report")
    };
    assert_eq!(identity, next);
    assert!(!success);
    next.generation += 1;
    next.operation_id = "removed".into();
    downloads.start(next.clone(), reference.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;
    tokio::time::timeout(Duration::from_secs(5), downloads.remove(&next))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), downloads.next_report())
            .await
            .is_err()
    );
    next.generation += 1;
    next.operation_id = "shutdown".into();
    downloads.start(next, reference);
    tokio::time::sleep(Duration::from_millis(100)).await;
    tokio::time::timeout(Duration::from_secs(5), downloads.shutdown())
        .await
        .unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    server.abort();
}

#[tokio::test]
async fn host_keeps_heartbeats_running_and_reports_download_without_launching() {
    let root = tempfile::tempdir().unwrap();
    let (reference, registry) = registry(false, Duration::from_secs(2)).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("ws://{}/register", listener.local_addr().unwrap());
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_diamant"))
        .args([
            "--cli",
            "--proxy-url",
            &proxy_url,
            "--host-name",
            "artifact-download-test",
            "--project-dir",
            root.path().to_str().unwrap(),
            "--artifact-dir",
            root.path().join("artifacts").to_str().unwrap(),
            "--artifact-plain-http",
            "--health-interval-seconds",
            "1",
        ])
        .env("RUST_LOG", "off")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        let _register = socket.next().await.unwrap().unwrap();
        let host = ResourceIdentity {
            namespace: "suntitle".into(),
            name: "artifact-download-test".into(),
            uid: "host".into(),
        };
        socket
            .send(Message::Text(
                serde_json::to_string(&Envelope::new(RegistrationServerMessage::Registered {
                    host,
                    session_id: "session".into(),
                }))
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                serde_json::to_string(&Envelope::new(
                    RegistrationServerMessage::DownloadArtifact {
                        identity: identity(),
                        reference,
                    },
                ))
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        let mut heartbeat = false;
        loop {
            let message = socket.next().await.unwrap().unwrap();
            let report: Envelope<RegistrationClientMessage> =
                serde_json::from_slice(&message.into_data()).unwrap();
            match report.message {
                RegistrationClientMessage::Heartbeat { .. } => heartbeat = true,
                RegistrationClientMessage::Acknowledgement {
                    operation,
                    success,
                    message,
                    ..
                } => {
                    assert!(heartbeat, "download blocked heartbeats");
                    assert_eq!(operation, Operation::Download);
                    assert!(success, "{message:?}");
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    registry.abort();
    let payloads = std::fs::read_dir(root.path().join("artifacts"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        std::fs::read(payloads[0].path().join("payload/run")).unwrap(),
        b"payload"
    );
}

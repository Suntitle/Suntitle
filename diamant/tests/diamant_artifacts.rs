use oci_artifact::constants::{
    ANNOTATION_UNPACK, IMAGE_LAYER_GZIP_MEDIA_TYPE, MediaType, OCI_IMAGE_MEDIA_TYPE,
    ORG_OPENCONTAINERS_IMAGE_TITLE,
};
use std::time::Duration;

use diamant::artifacts::{ArtifactDownloads, ArtifactOptions};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use sha2::{Digest, Sha256};
use suntitle_core::{
    kubernetes::{adapter::ResolvedAdapter, resources::ResourceIdentity},
    registration::*,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use warp::Filter;

/// Resolved configuration sent with the artifact, independent of adapter.yaml.
fn adapter() -> ResolvedAdapter {
    serde_json::from_value(json!({"nodeRole":"transcriber","execution":{"runtime":"executable","command":"./run"},"settings":{"language":"cs"},"arguments":[{"flag":"--language","source":"settings.language"}]})).unwrap()
}

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
    let manifest = json!({ "schemaVersion": 2, "mediaType": OCI_IMAGE_MEDIA_TYPE,
        "config": {"mediaType":MediaType::EmptyJSON.as_ref(), "digest":digest(b"{}"),"size":2},
        "layers":[{"mediaType":IMAGE_LAYER_GZIP_MEDIA_TYPE, "digest":digest(&blob), "size":blob.len(),
            "annotations":{(ORG_OPENCONTAINERS_IMAGE_TITLE):"payload",(ANNOTATION_UNPACK):"true"}}] });
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let route = warp::path::full().and_then(move |path: warp::path::FullPath| {
        let manifest = manifest.clone();
        let blob = blob.clone();
        async move {
            let (content, media) = if path.as_str().contains("/manifests/") {
                (manifest, OCI_IMAGE_MEDIA_TYPE)
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
        downloads.start(id.clone(), reference.clone(), adapter());
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
        if !corrupt {
            let installation = downloads
                .take_prepared(&id)
                .expect("installation must follow a successful report");
            assert_eq!(installation.identity, id);
            assert_eq!(installation.adapter.settings["language"], "cs");
            assert_eq!(
                installation.adapter.arguments[0].flag.as_deref(),
                Some("--language")
            );
            assert!(installation.root.join("run").is_file());
            assert_eq!(installation.working_directory, installation.root);
            assert!(downloads.take_prepared(&id).is_none());
            let mut manager = diamant::nodes::NodeProcessManager::new(
                root.path().into(),
                std::env::current_exe().unwrap(),
                "ws://localhost/".parse().unwrap(),
                Duration::from_secs(1),
            )
            .unwrap();
            let installed_root = installation.root.clone();
            server.abort();
            let mut next = id.clone();
            next.generation += 1;
            next.operation_id = "restore-cached-payload".into();
            downloads.start(next.clone(), reference, adapter());
            let report = tokio::time::timeout(Duration::from_secs(2), downloads.next_report())
                .await
                .unwrap();
            assert!(matches!(
                report,
                RegistrationClientMessage::Acknowledgement { success: true, .. }
            ));
            assert_eq!(downloads.take_prepared(&next).unwrap().root, installed_root);
            manager.prepare_extension(installation);
            assert!(manager.prepared_extension(&id).is_some());
            assert!(
                manager.snapshots().is_empty(),
                "preparation must not launch a process"
            );
            manager.remove(&id).await.unwrap();
            assert!(manager.prepared_extension(&id).is_none());
        } else {
            assert!(downloads.take_prepared(&id).is_none());
        }
        server.abort();
        let entries = std::fs::read_dir(root.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let entries: Vec<_> = entries
            .into_iter()
            .filter(|entry| entry.path().is_dir())
            .collect();
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
    downloads.start(id.clone(), reference.clone(), adapter());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut next = id.clone();
    next.generation += 1;
    next.operation_id = "replacement".into();
    downloads.start(next.clone(), "invalid reference".into(), adapter());
    downloads.start(id, reference.clone(), adapter());
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
    downloads.start(next.clone(), reference.clone(), adapter());
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
    downloads.start(next, reference, adapter());
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
                        adapter: Box::new(adapter()),
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
    let payloads: Vec<_> = payloads
        .into_iter()
        .filter(|entry| entry.path().is_dir())
        .collect();
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        std::fs::read(payloads[0].path().join("payload/run")).unwrap(),
        b"payload"
    );
}

/// Download success MUST NOT hide an invalid executable or leave a prepared record.
#[tokio::test]
async fn invalid_entry_point_reports_preparation_failure() {
    let root = tempfile::tempdir().unwrap();
    let mut downloads = ArtifactDownloads::new(ArtifactOptions {
        directory: Some(root.path().into()),
        plain_http: true,
    })
    .unwrap();
    let (reference, server) = registry(false, Duration::ZERO).await;
    let mut config = adapter();
    config.execution.command =
        suntitle_core::kubernetes::execution::ExecutionCommand::Executable("./missing".into());
    let id = identity();
    downloads.start(id.clone(), reference, config);
    let report = tokio::time::timeout(Duration::from_secs(10), downloads.next_report())
        .await
        .unwrap();
    let RegistrationClientMessage::Acknowledgement {
        success, message, ..
    } = report
    else {
        panic!("expected preparation result")
    };
    assert!(!success, "{message:?}");
    assert!(downloads.take_prepared(&id).is_none());
    downloads.shutdown().await;
    server.abort();
}

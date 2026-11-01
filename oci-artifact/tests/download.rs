use oci_artifact::constants::{
    ANNOTATION_UNPACK, IMAGE_LAYER_GZIP_MEDIA_TYPE, MediaType, OCI_IMAGE_MEDIA_TYPE,
    ORG_OPENCONTAINERS_IMAGE_TITLE,
};
use oci_artifact::{DownloadRequest, Downloader, Error, client::TransportOptions};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
struct Registry {
    address: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    requests: Arc<AtomicUsize>,
}
impl Registry {
    fn new(manifest: Value, blob: Vec<u8>, fail_first: bool) -> Self {
        Self::serve(manifest, blob, fail_first, false)
    }
    fn serve(manifest: Value, blob: Vec<u8>, fail_first: bool, interrupted: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let worker = thread::spawn(move || {
            let empty_json_media_type = MediaType::EmptyJSON;
            while !shutdown.load(Ordering::Relaxed) {
                let Ok((mut socket, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                loop {
                    let mut b = [0; 1024];
                    let n = socket.read(&mut b).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&b[..n]);
                    if request.windows(4).any(|s| s == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let path = request.split_whitespace().nth(1).unwrap_or("");
                let (status, content, media) = if path.contains("/manifests/") {
                    (200, manifest.as_slice(), OCI_IMAGE_MEDIA_TYPE)
                } else if path.ends_with(&digest(b"{}")) {
                    (200, &b"{}"[..], empty_json_media_type.as_ref())
                } else if path.contains("/blobs/") {
                    if count.fetch_add(1, Ordering::Relaxed) == 0 && fail_first && !interrupted {
                        (503, &b"temporary"[..], "text/plain")
                    } else {
                        (200, blob.as_slice(), "application/octet-stream")
                    }
                } else {
                    (200, &b"{}"[..], "application/json")
                };
                let header = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nContent-Type: {media}\r\nDocker-Content-Digest: {}\r\nConnection: close\r\n\r\n",
                    content.len(),
                    digest(content)
                );
                let _ = socket.write_all(header.as_bytes());
                if interrupted && path.contains("/blobs/") && count.load(Ordering::Relaxed) == 1 {
                    let _ = socket.write_all(&content[..content.len() / 2]);
                } else {
                    let _ = socket.write_all(content);
                }
            }
        });
        Self {
            address,
            stop,
            worker: Some(worker),
            requests,
        }
    }
    fn request(&self, path: &std::path::Path) -> DownloadRequest {
        DownloadRequest::new(format!("{}/fixture:latest", self.address), path)
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn archive() -> Vec<u8> {
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gzip);
    for (path, kind, data, mode, target) in [
        ("payload/", tar::EntryType::Directory, &b""[..], 0o751, None),
        (
            "payload/empty/",
            tar::EntryType::Directory,
            &b""[..],
            0o750,
            None,
        ),
        (
            "payload/nested/",
            tar::EntryType::Directory,
            &b""[..],
            0o750,
            None,
        ),
        (
            "payload/nested/é space",
            tar::EntryType::Regular,
            &b"hello"[..],
            0o640,
            None,
        ),
        (
            "payload/run",
            tar::EntryType::Regular,
            &b"executable"[..],
            0o751,
            None,
        ),
        (
            "payload/zero",
            tar::EntryType::Regular,
            &b""[..],
            0o640,
            None,
        ),
        (
            "payload/link",
            tar::EntryType::Symlink,
            &b""[..],
            0o777,
            Some("nested/é space"),
        ),
        (
            "payload/.wh.literal",
            tar::EntryType::Regular,
            &b"whiteout"[..],
            0o640,
            None,
        ),
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(kind);
        h.set_mode(mode);
        h.set_size(data.len() as u64);
        if let Some(t) = target {
            h.set_link_name(t).unwrap();
        }
        h.set_cksum();
        tar.append_data(&mut h, path, data).unwrap();
    }
    let binary: Vec<_> = (0..1024 * 1024).map(|n| (n % 251) as u8).collect();
    let mut h = tar::Header::new_gnu();
    h.set_mode(0o640);
    h.set_size(binary.len() as u64);
    h.set_cksum();
    tar.append_data(&mut h, "payload/binary", binary.as_slice())
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap()
}
fn manifest(blob: &[u8]) -> Value {
    json!({"schemaVersion":2,"mediaType":OCI_IMAGE_MEDIA_TYPE,"config":{"mediaType":MediaType::EmptyJSON.as_ref(),"size":2,"digest":digest(b"{}")},"layers":[{"mediaType":IMAGE_LAYER_GZIP_MEDIA_TYPE,"size":blob.len(),"digest":digest(blob),"annotations":{(ORG_OPENCONTAINERS_IMAGE_TITLE):"payload",(ANNOTATION_UNPACK):"true"}}]})
}
fn downloader() -> Downloader {
    Downloader::new(TransportOptions {
        plain_http: true,
        ..Default::default()
    })
    .unwrap()
}
#[tokio::test]
async fn directory_retry_and_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let blob = archive();
    let registry = Registry::new(manifest(&blob), blob, true);
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("parents/output");
    let request = registry.request(&output);
    let result = downloader().download(request).await.unwrap();
    assert_eq!(result.output_path, output);
    assert!(result.manifest_digest.starts_with("sha256:"));
    assert_eq!(
        std::fs::read(output.join("payload/nested/é space")).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read_link(output.join("payload/link"))
            .unwrap()
            .to_str(),
        Some("nested/é space")
    );
    assert_eq!(
        std::fs::metadata(output.join("payload/run"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
    assert_eq!(
        std::fs::metadata(output.join("payload/empty"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o750
    );
    assert_eq!(
        std::fs::metadata(output.join("payload/binary"))
            .unwrap()
            .len(),
        1024 * 1024
    );
    assert!(!output.join("payload/.wh.literal").exists());
    assert_eq!(std::fs::read_dir(&output).unwrap().count(), 1);
    assert_eq!(
        std::fs::read_dir(output.parent().unwrap()).unwrap().count(),
        1
    );
    assert_eq!(registry.requests.load(Ordering::Relaxed), 2);
}
#[tokio::test]
async fn failures_do_not_retry_or_write_unverified_content() {
    let root = tempfile::tempdir().unwrap();
    for case in ["size", "digest", "archive", "raw", "mixed"] {
        let blob = if case == "archive" {
            b"bad gzip".to_vec()
        } else {
            archive()
        };
        let mut m = manifest(&blob);
        match case {
            "size" => m["layers"][0]["size"] = json!(blob.len() + 1),
            "digest" => m["layers"][0]["digest"] = json!(digest(b"wrong")),
            "raw" => m["layers"][0]["annotations"] = json!({}),
            "mixed" => {
                let l = m["layers"][0].clone();
                m["layers"].as_array_mut().unwrap().push(l);
            }
            _ => (),
        }
        let registry = Registry::new(m, blob, false);
        let output = root.path().join(case);
        assert!(
            downloader()
                .download(registry.request(&output))
                .await
                .is_err(),
            "{case}"
        );
        assert_eq!(
            registry.requests.load(Ordering::Relaxed),
            if matches!(case, "raw" | "mixed") {
                0
            } else {
                1
            },
            "{case} MUST NOT retry"
        );
        if case == "archive" {
            assert!(output.is_dir());
            assert_eq!(std::fs::read_dir(&output).unwrap().count(), 0);
            std::fs::remove_dir(&output).unwrap();
        } else {
            assert!(!output.exists(), "{case}");
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
#[tokio::test]
async fn existing_and_cancelled() {
    let blob = archive();
    let registry = Registry::new(manifest(&blob), blob, false);
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("out");
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("keep"), "original").unwrap();
    downloader()
        .download(registry.request(&output))
        .await
        .unwrap();
    std::fs::write(output.join("payload/nested/é space"), b"local edit").unwrap();
    downloader()
        .download(registry.request(&output))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(output.join("payload/nested/é space")).unwrap(),
        b"hello"
    );
    let request = registry.request(&root.path().join("cancelled"));
    request.cancellation.cancel();
    assert!(matches!(
        downloader().download(request).await,
        Err(Error::Cancelled)
    ));
    assert_eq!(std::fs::read(output.join("keep")).unwrap(), b"original");
}
#[test]
fn missing_ca_file_is_a_configuration_error() {
    assert!(matches!(
        TransportOptions {
            ca_file: Some("/missing/ca.pem".into()),
            ..Default::default()
        }
        .build_config(),
        Err(Error::Configuration(_))
    ));
}

#[tokio::test]
async fn symlink_cannot_redirect_extraction_outside_destination() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gzip);
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_mode(0o777);
    h.set_size(0);
    h.set_link_name(&outside).unwrap();
    h.set_cksum();
    tar.append_data(&mut h, "payload/link", &b""[..]).unwrap();
    let mut h = tar::Header::new_gnu();
    h.set_mode(0o644);
    h.set_size(6);
    h.set_cksum();
    tar.append_data(&mut h, "payload/link/escape", &b"escape"[..])
        .unwrap();
    let blob = tar.into_inner().unwrap().finish().unwrap();
    let registry = Registry::new(manifest(&blob), blob, false);
    assert!(
        downloader()
            .download(registry.request(&root.path().join("output")))
            .await
            .is_err()
    );
    assert!(!outside.join("escape").exists());
    assert!(root.path().join("output").is_dir());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
}

#[tokio::test]
#[ignore = "requires ORAS 1.3.2"]
async fn oras_directory_round_trip() {
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        process::Command,
    };
    let version = Command::new("oras").arg("version").output().unwrap();
    assert!(
        String::from_utf8_lossy(&version.stdout)
            .lines()
            .any(|l| l.starts_with("Version:")
                && l.split_whitespace().nth(1).unwrap().starts_with("1.3.2"))
    );
    let root = tempfile::tempdir().unwrap();
    let payload = root.path().join("payload");
    std::fs::create_dir_all(payload.join("nested/empty")).unwrap();
    std::fs::write(payload.join("nested/é space"), b"hello").unwrap();
    std::fs::write(payload.join("zero"), b"").unwrap();
    std::fs::write(
        payload.join("binary"),
        (0..1024 * 1024)
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    symlink("nested/é space", payload.join("link")).unwrap();
    std::fs::set_permissions(&payload, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(
        payload.join("nested"),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    std::fs::set_permissions(
        payload.join("nested/é space"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let status = Command::new("oras")
        .current_dir(root.path())
        .args(["push", "--oci-layout", "layout:fixture", "payload"])
        .status()
        .unwrap();
    assert!(status.success());
    let index: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("layout/index.json")).unwrap())
            .unwrap();
    let manifest_digest = index["manifests"][0]["digest"].as_str().unwrap();
    let read_blob = |digest: &str| {
        std::fs::read(
            root.path()
                .join("layout/blobs/sha256")
                .join(digest.strip_prefix("sha256:").unwrap()),
        )
        .unwrap()
    };
    let m: Value = serde_json::from_slice(&read_blob(manifest_digest)).unwrap();
    let blob = read_blob(m["layers"][0]["digest"].as_str().unwrap());
    let registry = Registry::new(m, blob, false);
    let ours = root.path().join("ours");
    let oras = root.path().join("oras");
    downloader()
        .download(registry.request(&ours))
        .await
        .unwrap();
    let status = Command::new("oras")
        .args([
            "pull",
            "--plain-http",
            "--output",
            oras.to_str().unwrap(),
            &format!("{}/fixture:latest", registry.address),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    fn tree(
        root: &std::path::Path,
        path: &std::path::Path,
    ) -> std::collections::BTreeMap<std::path::PathBuf, (u32, Vec<u8>)> {
        let mut entries = std::collections::BTreeMap::new();
        for e in std::fs::read_dir(path).unwrap() {
            let p = e.unwrap().path();
            let m = p.symlink_metadata().unwrap();
            let data = if m.is_symlink() {
                std::fs::read_link(&p)
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes()
                    .to_vec()
            } else if m.is_file() {
                std::fs::read(&p).unwrap()
            } else {
                Vec::new()
            };
            entries.insert(
                p.strip_prefix(root).unwrap().into(),
                (
                    if m.is_symlink() {
                        0
                    } else {
                        m.permissions().mode() & 0o777
                    },
                    Sha256::digest(&data).to_vec(),
                ),
            );
            if m.is_dir() {
                entries.extend(tree(root, &p));
            }
        }
        entries
    }
    assert_eq!(tree(&ours, &ours), tree(&oras, &oras));
}

#[tokio::test]
async fn interrupted_transfer_retries_with_a_fresh_file() {
    let blob = archive();
    let registry = Registry::serve(manifest(&blob), blob, false, true);
    let root = tempfile::tempdir().unwrap();
    downloader()
        .download(registry.request(&root.path().join("out")))
        .await
        .unwrap();
    assert_eq!(registry.requests.load(Ordering::Relaxed), 2);
    assert_eq!(
        std::fs::metadata(root.path().join("out/payload/binary"))
            .unwrap()
            .len(),
        1024 * 1024
    );
}

#[tokio::test]
async fn cancellation_stops_download_before_restoration() {
    let blob = archive();
    let registry = Registry::serve(manifest(&blob), blob, false, true);
    let root = tempfile::tempdir().unwrap();
    let request = registry.request(&root.path().join("out"));
    let cancellation = request.cancellation.clone();
    let count = registry.requests.clone();
    let download = downloader();
    let task = tokio::spawn(async move { download.download(request).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while count.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap(),
        Err(Error::Cancelled)
    ));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn parent_paths_cannot_escape_destination() {
    let root = tempfile::tempdir().unwrap();
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gzip);
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(6);
    let path = b"../../escape";
    header.as_mut_bytes()[..path.len()].copy_from_slice(path);
    header.set_cksum();
    tar.append(&header, &b"escape"[..]).unwrap();
    let blob = tar.into_inner().unwrap().finish().unwrap();
    let registry = Registry::new(manifest(&blob), blob, false);
    let output = root.path().join("output");
    let _ = downloader().download(registry.request(&output)).await;
    assert!(!root.path().join("escape").exists());
    assert!(!output.join("escape").exists());
    assert!(std::fs::read_dir(root.path()).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".oci-artifact-")
    }));
}

#[tokio::test]
async fn raw_files_preserve_bytes_and_names() {
    for (name, bytes) in [
        ("text.txt", b"hello\n".to_vec()),
        (
            "nested/é binary",
            (0..1024 * 1024).map(|n| (n % 256) as u8).collect(),
        ),
        ("empty", Vec::new()),
    ] {
        let mut m = manifest(&bytes);
        m["layers"][0]["annotations"] = json!({(ORG_OPENCONTAINERS_IMAGE_TITLE):name});
        let registry = Registry::new(m, bytes.clone(), false);
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("out");
        downloader()
            .download(registry.request(&output))
            .await
            .unwrap();
        assert_eq!(std::fs::read(output.join(name)).unwrap(), bytes);
        std::fs::write(output.join(name), b"local edit longer than an empty file").unwrap();
        std::fs::write(output.join("keep"), b"unrelated").unwrap();
        downloader()
            .download(registry.request(&output))
            .await
            .unwrap();
        assert_eq!(std::fs::read(output.join(name)).unwrap(), bytes);
        assert_eq!(std::fs::read(output.join("keep")).unwrap(), b"unrelated");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[tokio::test]
async fn raw_file_names_cannot_escape_destination() {
    for name in ["../escape", "/escape", "nested/../../escape", ".", ""] {
        let bytes = b"bytes".to_vec();
        let mut m = manifest(&bytes);
        m["layers"][0]["annotations"] = json!({(ORG_OPENCONTAINERS_IMAGE_TITLE):name});
        let registry = Registry::new(m, bytes, false);
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            downloader()
                .download(registry.request(&root.path().join("out")))
                .await,
            Err(Error::Unsupported(_))
        ));
        assert_eq!(registry.requests.load(Ordering::Relaxed), 0);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
#[ignore = "requires ORAS 1.3.2"]
async fn oras_single_file_round_trip() {
    use std::{os::unix::fs::PermissionsExt, process::Command};

    let version = Command::new("oras").arg("version").output().unwrap();
    assert!(
        String::from_utf8_lossy(&version.stdout)
            .lines()
            .any(|line| {
                line.starts_with("Version:")
                    && line.split_whitespace().nth(1).unwrap().starts_with("1.3.2")
            })
    );
    for (name, media, bytes) in [
        ("text.txt", "text/plain", b"plain text\n".to_vec()),
        (
            "binary",
            "application/octet-stream",
            (0..1024 * 1024).map(|n| (n % 256) as u8).collect(),
        ),
        (
            "nested/é space",
            "application/octet-stream",
            b"\0\xff\x80\n".to_vec(),
        ),
        ("empty", "application/octet-stream", Vec::new()),
    ] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join(name);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, &bytes).unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o751)).unwrap();
        let pushed = Command::new("oras")
            .current_dir(root.path())
            .args([
                "push",
                "--oci-layout",
                "layout:fixture",
                &format!("{name}:{media}"),
            ])
            .output()
            .unwrap();
        assert!(
            pushed.status.success(),
            "{}",
            String::from_utf8_lossy(&pushed.stderr)
        );
        let index: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("layout/index.json")).unwrap())
                .unwrap();
        let read_blob = |digest: &str| {
            std::fs::read(
                root.path()
                    .join("layout/blobs/sha256")
                    .join(digest.strip_prefix("sha256:").unwrap()),
            )
            .unwrap()
        };
        let m: Value = serde_json::from_slice(&read_blob(
            index["manifests"][0]["digest"].as_str().unwrap(),
        ))
        .unwrap();
        assert!(m["layers"][0]["annotations"][ANNOTATION_UNPACK].is_null());
        let blob = read_blob(m["layers"][0]["digest"].as_str().unwrap());
        assert_eq!(blob, bytes);
        let registry = Registry::new(m, blob, false);
        let ours = root.path().join("ours");
        let oras = root.path().join("oras");
        downloader()
            .download(registry.request(&ours))
            .await
            .unwrap();
        let pulled = Command::new("oras")
            .args([
                "pull",
                "--plain-http",
                "--output",
                oras.to_str().unwrap(),
                &format!("{}/fixture:latest", registry.address),
            ])
            .output()
            .unwrap();
        assert!(
            pulled.status.success(),
            "{}",
            String::from_utf8_lossy(&pulled.stderr)
        );
        assert_eq!(std::fs::read(ours.join(name)).unwrap(), bytes);
        assert_eq!(std::fs::read(oras.join(name)).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(ours.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            std::fs::metadata(oras.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        );
        assert_eq!(std::fs::read_dir(&ours).unwrap().count(), 1);
    }
}

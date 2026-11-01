//! Live compatibility check against the Distribution service in compose.yaml.
use std::{
    collections::BTreeMap,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use oci_artifact::{DownloadRequest, Downloader, client::TransportOptions};
use sha2::{Digest, Sha256};

/// Run the pinned reference client and retain its diagnostics on failure.
fn oras(directory: &Path, arguments: &[&str]) {
    let output = Command::new("oras")
        .current_dir(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "oras {arguments:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Compare entry types, ordinary modes, file hashes, and relative link targets.
fn tree(root: &Path, path: &Path) -> BTreeMap<PathBuf, (&'static str, u32, Vec<u8>)> {
    let mut entries = BTreeMap::new();
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        let metadata = path.symlink_metadata().unwrap();
        let (kind, mode, data) = if metadata.is_symlink() {
            (
                "symlink",
                0,
                std::fs::read_link(&path)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            )
        } else if metadata.is_dir() {
            (
                "directory",
                metadata.permissions().mode() & 0o777,
                Vec::new(),
            )
        } else {
            (
                "file",
                metadata.permissions().mode() & 0o777,
                Sha256::digest(std::fs::read(&path).unwrap()).to_vec(),
            )
        };
        entries.insert(path.strip_prefix(root).unwrap().into(), (kind, mode, data));
        if metadata.is_dir() {
            entries.extend(tree(root, &path));
        }
    }

    entries
}

#[tokio::test]
#[ignore = "requires the Compose Distribution route and ORAS 1.3.2"]
async fn oras_push_and_library_pull_through_compose_distribution() {
    let version = Command::new("oras").arg("version").output().unwrap();
    assert!(
        String::from_utf8_lossy(&version.stdout)
            .lines()
            .any(|line| {
                line.starts_with("Version:")
                    && line.split_whitespace().nth(1).unwrap().starts_with("1.3.2")
            })
    );
    let registry =
        std::env::var("OCI_ARTIFACT_TEST_REGISTRY").unwrap_or_else(|_| "registry.localhost".into());
    let run = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory_reference = format!("{registry}/oci-artifact-tests/{run}/directory:fixture");
    let file_reference = format!("{registry}/oci-artifact-tests/{run}/file:fixture");
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let payload = source.join("payload");
    std::fs::create_dir_all(payload.join("nested/empty")).unwrap();
    std::fs::write(payload.join("nested/é space"), b"text fixture\n").unwrap();
    std::fs::write(payload.join("empty-file"), b"").unwrap();
    std::fs::write(payload.join("run"), b"#!/bin/sh\nexit 0\n").unwrap();
    let binary: Vec<_> = (0..1024 * 1024).map(|n| (n % 256) as u8).collect();
    std::fs::write(payload.join("binary"), &binary).unwrap();
    std::fs::write(source.join("standalone.bin"), &binary).unwrap();
    symlink("nested/é space", payload.join("link")).unwrap();
    for (path, mode) in [
        (payload.clone(), 0o755),
        (payload.join("nested"), 0o750),
        (payload.join("nested/empty"), 0o750),
        (payload.join("nested/é space"), 0o640),
        (payload.join("binary"), 0o640),
        (payload.join("run"), 0o751),
        (source.join("standalone.bin"), 0o751),
    ] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    oras(
        &source,
        &["push", "--plain-http", &directory_reference, "payload"],
    );
    oras(
        &source,
        &[
            "push",
            "--plain-http",
            &file_reference,
            "standalone.bin:application/octet-stream",
        ],
    );
    let downloader = Downloader::new(TransportOptions {
        plain_http: true,
        ..Default::default()
    })
    .unwrap();
    for (name, reference) in [
        ("directory", &directory_reference),
        ("file", &file_reference),
    ] {
        let ours = root.path().join(format!("library-{name}"));
        let reference_output = root.path().join(format!("oras-{name}"));
        let result = downloader
            .download(DownloadRequest::new(reference, &ours))
            .await
            .unwrap();
        oras(
            root.path(),
            &[
                "pull",
                "--plain-http",
                "--output",
                reference_output.to_str().unwrap(),
                reference,
            ],
        );
        assert_eq!(
            tree(&ours, &ours),
            tree(&reference_output, &reference_output)
        );
        assert_eq!(std::fs::read_dir(&ours).unwrap().count(), 1);
        if name == "directory" {
            assert_eq!(tree(&ours, &ours.join("payload")), tree(&source, &payload));
        } else {
            assert_eq!(std::fs::read(ours.join("standalone.bin")).unwrap(), binary);
        }
        println!("Verified {reference}: {}", result.manifest_digest);
    }
    assert!(std::fs::read_dir(root.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".oci-artifact-")
    }));
}

//! HTTP(S) sources become private depth-one bare repositories, then tar streams.
//! The fetched commit is pinned for packaging. Dropping a snapshot removes its objects.
use oci_artifact::{CancellationToken, RegistryAuth};
use std::{
    io,
    io::Write,
    num::NonZeroU32,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Private object database retained only until archive production finishes.
pub(super) struct Snapshot {
    /// RAII owner of the bare repository. No working checkout is created.
    directory: tempfile::TempDir,
    /// Immutable commit selected by the remote's default HEAD during fetch.
    commit: gix::ObjectId,
}
impl Snapshot {
    /// Fetch HTTP(S) HEAD without tags or ancestors, outside the async runtime.
    pub async fn fetch(
        source: &str,
        authentication: Option<&RegistryAuth>,
        cancellation: &CancellationToken,
    ) -> Result<Self, suntitle_core::kubernetes::Error> {
        let url = url::Url::parse(source).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Git sources require an HTTP(S) URL; SSH is not supported",
            )
        })?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Git sources require HTTP(S) without embedded credentials; SSH is not supported",
            )
            .into());
        }
        let overrides = vec![
            "credential.helper=".to_string(),
            "gitoxide.credentials.terminalPrompt=false".into(),
        ];
        let mut headers = Vec::new();
        if let Some(RegistryAuth::Basic(username, password)) = authentication {
            use base64::Engine;
            headers.push(format!(
                "Authorization: Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
            ));
        }
        let interrupted = Arc::new(AtomicBool::new(cancellation.is_cancelled()));
        let signal = interrupted.clone();
        let token = cancellation.clone();
        let watcher = tokio::spawn(async move {
            token.cancelled().await;
            signal.store(true, Ordering::Relaxed);
        });
        let source = url.to_string();
        let result = tokio::task::spawn_blocking(
            move || -> Result<Self, suntitle_core::kubernetes::Error> {
                if interrupted.load(Ordering::Relaxed) {
                    return Err(io::Error::other("Git fetch cancelled").into());
                }
                let directory = tempfile::Builder::new().prefix("suntitle-git-").tempdir()?;
                let mut clone = gix::clone::PrepareFetch::new(
                    source.as_str(),
                    directory.path(),
                    gix::create::Kind::Bare,
                    gix::create::Options::default(),
                    gix::open::Options::isolated(),
                )?
                .with_revision(Some("HEAD"))?
                .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(
                    NonZeroU32::new(1).unwrap(),
                ))
                .with_in_memory_config_overrides(overrides)
                .configure_connection(move |connection| {
                    use gix::protocol::transport::client::blocking_io::http;
                    let backend = http::reqwest::Options {
                        configure_request: Some(Box::new(|request| {
                            *request.timeout_mut() = Some(std::time::Duration::from_secs(60));
                            Ok(())
                        })),
                        ..Default::default()
                    };
                    connection.set_transport_options(Box::new(http::Options {
                        extra_headers: headers.clone(),
                        backend: Some(Arc::new(std::sync::Mutex::new(backend))),
                        ..Default::default()
                    }));
                    Ok(())
                });
                let (repository, _) = clone.fetch_only(gix::progress::Discard, &interrupted)?;
                let commit = repository.head_commit()?.id;
                drop(repository);
                Ok(Self { directory, commit })
            },
        )
        .await;
        watcher.abort();
        let _ = watcher.await;
        let snapshot = result??;
        if cancellation.is_cancelled() {
            return Err(io::Error::other("Git fetch cancelled").into());
        }
        Ok(snapshot)
    }

    /// Append the pinned tree directly from objects, preserving executable and link modes.
    pub fn append<W: Write>(
        &self,
        archive: &mut tar::Builder<W>,
        cancellation: &CancellationToken,
    ) -> io::Result<()> {
        let repository = gix::open_opts(self.directory.path(), gix::open::Options::isolated())
            .map_err(io::Error::other)?;
        let commit = repository
            .find_object(self.commit)
            .map_err(io::Error::other)?
            .try_into_commit()
            .map_err(io::Error::other)?;
        let tree = commit.tree().map_err(io::Error::other)?;
        append_tree(
            archive,
            &repository,
            &tree,
            Path::new("extension"),
            cancellation,
        )
    }
}

/// Walk Git objects without a working checkout or including repository metadata.
fn append_tree<W: Write>(
    archive: &mut tar::Builder<W>,
    repository: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &Path,
    cancellation: &CancellationToken,
) -> io::Result<()> {
    for entry in tree.iter() {
        if cancellation.is_cancelled() {
            return Err(io::Error::other("Git packaging cancelled"));
        }
        let entry = entry.map_err(io::Error::other)?;
        let name = std::str::from_utf8(entry.filename()).map_err(io::Error::other)?;
        if name == ".git" || (prefix == Path::new("extension") && name == "adapter.yaml") {
            continue;
        }
        if name.is_empty()
            || !Path::new(name)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(io::Error::other("invalid Git tree path"));
        }
        let path = prefix.join(name);
        let mode = entry.mode().value();
        let object = repository
            .find_object(entry.object_id())
            .map_err(io::Error::other)?;
        if mode == 0o040000 {
            append_tree(
                archive,
                repository,
                &object.try_into_tree().map_err(io::Error::other)?,
                &path,
                cancellation,
            )?;
        } else {
            let mut header = tar::Header::new_gnu();
            header.set_mode(if mode == 0o100755 { 0o755 } else { 0o644 });
            match mode {
                0o100644 | 0o100755 => {
                    header.set_size(object.data.len() as u64);
                    header.set_cksum();
                    archive.append_data(&mut header, &path, object.data.as_slice())?;
                }
                0o120000 => {
                    let link = std::str::from_utf8(&object.data).map_err(io::Error::other)?;
                    let link = Path::new(link);
                    if link.is_absolute()
                        || link
                            .components()
                            .any(|part| matches!(part, std::path::Component::ParentDir))
                    {
                        return Err(io::Error::other("Git symlink MUST stay inside the payload"));
                    }
                    header.set_entry_type(tar::EntryType::Symlink);
                    archive.append_link(&mut header, &path, link)?;
                }
                _ => return Err(io::Error::other("Git submodules are not supported")),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use warp::Filter;

    /// Run the Git reference implementation only to prepare test inputs.
    fn git(directory: &Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(directory)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    /// Serve Git's smart HTTP backend; no registry emulator or external service is needed.
    async fn serve(root: &Path) -> (String, tokio::task::JoinHandle<()>) {
        let root = root.to_owned();
        let route = warp::method()
            .and(warp::path::full())
            .and(warp::query::raw().or(warp::any().map(String::new)).unify())
            .and(warp::header::optional::<String>("git-protocol"))
            .and(warp::body::bytes())
            .map(
                move |method: warp::http::Method,
                      path: warp::path::FullPath,
                      query: String,
                      protocol: Option<String>,
                      body: actix_web::web::Bytes| {
                    let mut child = Command::new("git")
                        .arg("http-backend")
                        .env("GIT_PROJECT_ROOT", &root)
                        .env("GIT_HTTP_EXPORT_ALL", "1")
                        .env("PATH_INFO", path.as_str())
                        .env("QUERY_STRING", query)
                        .env("REQUEST_METHOD", method.as_str())
                        .env("CONTENT_TYPE", "application/x-git-upload-pack-request")
                        .env("CONTENT_LENGTH", body.len().to_string())
                        .env("HTTP_GIT_PROTOCOL", protocol.unwrap_or_default())
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .spawn()
                        .unwrap();
                    child.stdin.take().unwrap().write_all(&body).unwrap();
                    let output = child.wait_with_output().unwrap();
                    assert!(output.status.success());
                    let split = output
                        .stdout
                        .windows(4)
                        .position(|bytes| bytes == b"\r\n\r\n")
                        .unwrap();
                    let headers = String::from_utf8_lossy(&output.stdout[..split]);
                    let media = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Type: "))
                        .unwrap();
                    warp::http::Response::builder()
                        .header("Content-Type", media)
                        .body(output.stdout[split + 4..].to_vec())
                        .unwrap()
                },
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        (
            format!("http://{address}/repo/.git"),
            tokio::spawn(warp::serve(route).incoming(listener).run()),
        )
    }

    /// Only HEAD's snapshot is retained, permissions survive, and dropping it removes objects.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shallow_http_snapshot_packages_without_checkout_and_cleans_up() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("repo");
        std::fs::create_dir(&work).unwrap();
        git(&work, &["init", "-b", "main"]);
        std::fs::write(work.join("plugin"), b"old").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "old"]);
        let ancestor = git(&work, &["rev-parse", "HEAD"]);
        git(&work, &["tag", "old-tag"]);
        std::fs::write(work.join("plugin"), b"#!/bin/sh\nexit 0\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["update-index", "--chmod=+x", "plugin"]);
        git(&work, &["commit", "-m", "latest"]);
        let latest = git(&work, &["rev-parse", "HEAD"]);
        let (url, server) = serve(root.path()).await;
        let snapshot = Snapshot::fetch(&url, None, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(snapshot.commit.to_string(), latest);
        let path = snapshot.directory.path().to_owned();
        assert!(!path.join(".git").exists());
        assert!(!path.join("plugin").exists());
        let repository = gix::open(&path).unwrap();
        assert!(
            repository
                .find_object(gix::ObjectId::from_hex(ancestor.as_bytes()).unwrap())
                .is_err()
        );
        assert!(repository.find_reference("refs/tags/old-tag").is_err());
        drop(repository);
        let token = CancellationToken::new();
        let mut archive = tar::Builder::new(Vec::new());
        snapshot.append(&mut archive, &token).unwrap();
        let bytes = archive.into_inner().unwrap();
        let mut archive = tar::Archive::new(bytes.as_slice());
        let mut entries = archive.entries().unwrap();
        let entry = entries.next().unwrap().unwrap();
        assert_eq!(entry.path().unwrap(), Path::new("extension/plugin"));
        assert_eq!(entry.header().mode().unwrap(), 0o755);
        assert!(entries.next().is_none());
        token.cancel();
        assert!(
            snapshot
                .append(&mut tar::Builder::new(Vec::new()), &token)
                .is_err()
        );
        drop(snapshot);
        assert!(!path.exists());
        server.abort();
    }

    /// Unsupported schemes and pre-cancelled work fail without contacting the source.
    #[tokio::test]
    async fn rejects_ssh_and_cancelled_fetches() {
        let token = CancellationToken::new();
        for source in [
            "ssh://git@example.invalid/repo",
            "git@example.invalid:repo",
            "file:///repo",
        ] {
            assert!(Snapshot::fetch(source, None, &token).await.is_err());
        }
        token.cancel();
        assert!(
            Snapshot::fetch("http://127.0.0.1:1/repo", None, &token)
                .await
                .is_err()
        );
    }
}

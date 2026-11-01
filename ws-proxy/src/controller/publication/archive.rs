//! Archive production owns the source until its bytes and metadata are complete.
//! PreparedUpload joins the producer after success, failure, or cancellation.
use oci_artifact::{CancellationToken, PreparedUpload};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::{Path, PathBuf},
};
use tokio_util::io::{ReaderStream, SyncIoBridge};

/// Writer forwarding a bounded archive stream while hashing uncompressed bytes.
struct DigestWriter<W> {
    /// Gzip output feeding the registry upload.
    output: W,
    /// ORAS uncompressed tar digest calculated incrementally.
    hash: sha2::Sha256,
}
impl<W: Write> Write for DigestWriter<W> {
    /// Hash only bytes successfully accepted by the output.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        use sha2::Digest;
        let count = self.output.write(bytes)?;
        self.hash.update(&bytes[..count]);
        Ok(count)
    }
    /// Flush without affecting the digest.
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}
/// Filesystem tree or pinned Git objects owned by one archive producer.
pub(super) enum DirectorySource {
    /// Existing HUB directory, retained by the caller.
    Local(PathBuf),
    /// Private temporary bare repository, removed after packaging.
    Git(super::git::Snapshot),
}
/// Create a bounded gzip/tar producer and defer its annotations until stream completion.
pub(super) fn prepare(
    source: DirectorySource,
    adapter: Vec<u8>,
    cancellation: CancellationToken,
) -> PreparedUpload {
    let (read, write) = tokio::io::duplex(64 * 1024);
    let producer_cancellation = cancellation.clone();
    let producer = tokio::task::spawn_blocking(move || {
        let cancellation = producer_cancellation;
        use sha2::Digest;
        let gzip =
            flate2::write::GzEncoder::new(SyncIoBridge::new(write), flate2::Compression::default());
        let mut archive = tar::Builder::new(DigestWriter {
            output: gzip,
            hash: sha2::Sha256::new(),
        });
        archive.follow_symlinks(false);
        match &source {
            DirectorySource::Local(path) => {
                append_directory(&mut archive, path, Path::new("extension"), &cancellation)?
            }
            DirectorySource::Git(snapshot) => snapshot.append(&mut archive, &cancellation)?,
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(adapter.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, "extension/adapter.yaml", adapter.as_slice())?;
        let DigestWriter { output, hash } = archive.into_inner()?;
        let mut output = output.finish()?;
        output.flush()?;
        Ok(BTreeMap::from([(
            oci_artifact::constants::ANNOTATION_DIGEST.into(),
            format!("sha256:{:x}", hash.finalize()),
        )]))
    });
    PreparedUpload::directory(Box::pin(ReaderStream::new(read)), producer, cancellation)
}
/// Preserve permissions and safe links while omitting source metadata and the replaced adapter.
fn append_directory<W: Write>(
    archive: &mut tar::Builder<W>,
    source: &Path,
    target: &Path,
    cancellation: &CancellationToken,
) -> io::Result<()> {
    if cancellation.is_cancelled() {
        return Err(io::Error::other("publication cancelled"));
    }
    let metadata = source.symlink_metadata()?;
    let mut header = tar::Header::new_gnu();
    header.set_metadata(&metadata);
    header.set_uid(0);
    header.set_gid(0);
    if metadata.is_dir() {
        archive.append_data(&mut header, target, io::empty())?;
        let mut entries = std::fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if entry.file_name() == ".git"
                || (target == Path::new("extension") && entry.file_name() == "adapter.yaml")
            {
                continue;
            }
            append_directory(
                archive,
                &entry.path(),
                &target.join(entry.file_name()),
                cancellation,
            )?;
        }
    } else if metadata.is_file() {
        archive.append_data(&mut header, target, std::fs::File::open(source)?)?;
    } else if metadata.is_symlink() {
        let link = std::fs::read_link(source)?;
        if link.is_absolute()
            || link
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(io::Error::other(
                "extension symlink MUST stay inside the payload",
            ));
        }
        archive.append_link(&mut header, target, link)?;
    } else {
        return Err(io::Error::other("unsupported source filesystem entry"));
    }
    Ok(())
}

# oci-artifact

Push or download a single file or directory artifact in an OCI registry. Restore
directory contents, symlinks, and ordinary archived permissions locally.
Diamant can import this library without a CLI or an installed ORAS binary.

```rust,no_run
use oci_artifact::{DownloadRequest, Downloader, RegistryAuth, client::TransportOptions};

# async fn pull() -> Result<(), oci_artifact::Error> {
let downloader = Downloader::new(TransportOptions::default())?;
let mut request = DownloadRequest::new("registry.example.com/assets:v1", "downloads/assets");
request.authentication = RegistryAuth::Basic("username".into(), "password".into());
let result = downloader.download(request).await?;
# Ok(())
# }
```

Publish prepared streams with explicit transport and credential options:

```rust,no_run
use oci_artifact::{CancellationToken, PreparedUpload, Publisher, RegistryAuth, client::TransportOptions};

# async fn push() -> Result<(), oci_artifact::Error> {
let publisher = Publisher::new(TransportOptions::default())?;
let content = Box::pin(futures_util::stream::once(async {
    Ok(tokio_util::bytes::Bytes::from_static(b"payload"))
}));
let upload = PreparedUpload::file("payload.txt".into(), content);
let reference = publisher.publish(
    "registry.example.com/assets:v1",
    &RegistryAuth::Anonymous,
    upload,
    &CancellationToken::new(),
).await?;
println!("Published {reference}");
# Ok(())
# }
```

HTTP downloads, Git checkouts, local source selection, and directory packaging
remain caller-owned. Pass prepared streams to `Publisher` as `PreparedUpload`.
File layers use ORAS's default `application/vnd.oci.image.layer.v1.tar` media type
without unpack annotations. Directory streams MUST contain a gzip/tar archive
with an `extension` directory prefix. The archive producer returns the
uncompressed tar SHA-256 in `io.deis.oras.content.digest`, following the
[ORAS file store](https://github.com/oras-project/oras-go/tree/v2/content/file).
The config is unnamed OCI empty JSON metadata.
This is an artifact manifest, not a runnable container image.

Cancellation uses an explicit token. Cancel and await completion so producers
can stop and clean up. Cancelled or failed publications can leave unreferenced
registry blobs. A failure or cancellation during manifest publication can leave
the tag published. Publications do not currently retry failed transfers automatically.

Credentials are explicit; requests default to anonymous access. Transport uses
HTTPS and verified certificates. `TransportOptions` also accepts plain HTTP,
a certificate verification bypass, or additional PEM certificate roots.

The artifact contains one named layer. A file layer is saved byte-for-byte under
its `org.opencontainers.image.title` filename, including nested relative paths.
Binary and empty files are supported. Media types do not trigger extraction.

A layer marked `io.deis.oras.content.unpack=true` is a gzip directory archive.
ocirender extracts its existing directory prefix: `payload/file` becomes
`downloads/assets/payload/file` in the example above. Empty config metadata is
accepted without parsing it as a container runtime configuration. Multiple
layers, mixed pushes, and image indexes are unsupported.

The destination may already exist. The library downloads a private blob and checks
its size and digest before restoring directly into the destination. Existing files
are overwritten; unrelated entries remain. Directory extraction uses ocirender's
standard tar overwrite behavior. Conflicting directory types may fail extraction.
Raw files replace terminal symlinks without writing through them and use atomic
exclusive creation to prevent following a newly introduced terminal symlink.
Updates are not atomic: extraction or local I/O failures may leave partial output.
Transient network failures receive bounded retries; corrupt data and local failures
do not.

To cancel, clone `request.cancellation`, call `cancel()`, and await the download.
Cancellation during extraction waits for the renderer. Partial output remains.
Callers must not abort the task or drop its future while extraction is running.
The result contains the manifest digest and absolute output path.

On Unix, directory archives retain ordinary file and directory modes, including executable
bits, through ocirender 0.3.0. Direct file pushes do not record Unix permissions;
like ORAS, downloaded raw files use normal file-creation permissions and the
process umask. Ownership, privileged modes, xattrs, and full metadata fidelity are not
promised. Within directory archives, literal `.wh.*` filenames are interpreted as
container whiteouts and do not round-trip. No extraction workarounds are applied.

Linux and macOS use upstream ocirender. Windows uses conditionally compiled
`tar`/`flate2` extraction for the supported single gzip directory layer;
ocirender is excluded from Windows builds. Windows permissions follow native
filesystem behavior; restoring symlinks requires host privileges.
Default Rust TLS supports static Linux musl builds. XZ is compiled from source
with static linking enabled. Native TLS is selectable with
`--no-default-features --features native-tls`.

Run `cargo test --manifest-path oci-artifact/Cargo.toml` for local registry tests.
The optional ORAS 1.3.2 compatibility tests run with
`cargo test --manifest-path oci-artifact/Cargo.toml --test download oras_ -- --ignored`.
ORAS is used only as a test reference, never by the library.

To verify downloads of ORAS pushes through the Compose Distribution service:

```sh
docker compose up -d --no-deps distribution traefik
cargo test --manifest-path oci-artifact/Cargo.toml --test compose_distribution -- --ignored --nocapture
```

This test requires ORAS 1.3.2. ORAS pushes generated directory, binary-file,
empty-file, and empty-directory fixtures. Both clients pull the results; the
test compares paths, contents, ordinary modes, and symlink targets.
The default endpoint is `registry.localhost`;
override it with `OCI_ARTIFACT_TEST_REGISTRY` when using a different registry host.
Each run uses a unique `oci-artifact-tests/` repository prefix and prints the
references and manifest digests. Registry fixtures remain available for inspection;
local fixture and output directories are removed after the test.

`Publisher` publishes one `PreparedUpload` with chunked uploads.
It applies backpressure without staging a complete payload. Empty files and
responses without a known size are supported. HTTP responses publish as one named file. Directory archives retain the existing
ORAS convention. The prepared upload owns its stream and optional archive producer, collects
metadata computed during streaming, and joins the producer during publication
or cancellation cleanup. Publication completes only after the manifest is accepted and its
canonical digest reference resolves through the OCI client.

`Publisher::republish` streams upstream blobs directly into a destination registry
and republishes the original manifest bytes. It does not inspect extension layout,
adapter contents, or payload files. Library-owned digest verification still applies.

HTTP executable file layers carry `io.suntitle.executable: "true"`. The downloader
restores these files with mode `0755` on Unix hosts.

`DownloadResult::payload_path` identifies the restored file or directory using its
OCI layer filename annotation. Consumers can resolve installation paths without
guessing a directory prefix from extracted files.

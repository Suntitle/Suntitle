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

Push prepared content with the same transport and credential options:

```rust,no_run
use oci_artifact::{UploadRequest, Uploader, client::TransportOptions};

# async fn push() -> Result<(), oci_artifact::Error> {
let uploader = Uploader::new(TransportOptions::default())?;
let request = UploadRequest::new("registry.example.com/assets:v1", "prepared/payload");
let result = uploader.upload(request).await?;
println!("Published {}", result.manifest_digest);
# Ok(())
# }
```

HTTP downloads, Git checkouts, and local source selection remain caller-owned.
Pass their prepared file or directory to `Uploader`. Uploads use the source's
UTF-8 basename as the title and contain one layer, compatible with `Downloader`
and `oras pull`. Directory layers contain a gzip/tar archive with its directory
prefix, `io.deis.oras.content.unpack=true`, and the uncompressed tar SHA-256 in
`io.deis.oras.content.digest`, following the
[ORAS file store](https://github.com/oras-project/oras-go/tree/v2/content/file).
Raw files use ORAS's default `application/vnd.oci.image.layer.v1.tar` media type
without unpack annotations. The config is unnamed OCI empty JSON metadata.
This is an artifact manifest, not a runnable container image.

Packaging and upload use bounded buffers and private temporary disk storage.
Nonempty payloads and config blobs use chunked streaming uploads; the registry
client computes their digests and sizes during transfer. Empty files use the
client's ordinary blob upload because its chunked streaming API rejects empty
streams. The public upload API still accepts local filesystem paths.
Keep the source stable until upload returns. Files, directories, and archived
symlinks are supported; top-level symlinks and special files are rejected.
Directory archives normalize ownership to zero and retain ordinary permissions.
Archive bytes and manifest digests need not equal ORAS's encoding.
Upload references must use a tag (omitting it means `latest`). The result contains
the immutable manifest digest. Credentials are explicit and cancellation uses
the request token. Cancel and await completion so packaging can stop and clean up.
Cancelled or failed uploads can leave unreferenced registry blobs. A failure or
cancellation during manifest publication can leave the tag published.
Uploads do not currently retry failed transfers automatically.

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

Directory archives retain ordinary file and directory modes, including executable
bits, through ocirender 0.3.0. Direct file pushes do not record Unix permissions;
like ORAS, downloaded raw files use normal file-creation permissions and the
process umask. Ownership, privileged modes, xattrs, and full metadata fidelity are not
promised. Within directory archives, literal `.wh.*` filenames are interpreted as
container whiteouts and do not round-trip. No extraction workarounds are applied.

Linux and macOS are supported. Windows is unsupported by the upstream renderer.
Default Rust TLS supports static Linux musl builds. XZ is compiled from source
with static linking enabled. Native TLS is selectable with
`--no-default-features --features native-tls`.

Run `cargo test --manifest-path oci-artifact/Cargo.toml` for local registry tests.
The optional ORAS 1.3.2 compatibility tests run with
`cargo test --manifest-path oci-artifact/Cargo.toml --test download oras_ -- --ignored`.
ORAS is used only as a test reference, never by the library.

To verify real pushes through the Compose Distribution service:

```sh
docker compose up -d --no-deps distribution traefik
cargo test --manifest-path oci-artifact/Cargo.toml --test compose_distribution -- --ignored --nocapture
```

This test requires ORAS 1.3.2. Both ORAS and the library push generated directory,
binary-file, empty-file, and empty-directory fixtures. Both clients pull the
results; the test compares paths, contents, ordinary modes, and symlink targets,
and checks the library's returned digest by pulling its immutable reference.
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

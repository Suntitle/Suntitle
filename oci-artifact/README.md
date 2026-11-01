# oci-artifact

Download a single file or directory artifact from an OCI registry. Restore
directory contents, symlinks, and ordinary archived permissions locally.
Diamant can import this library without a CLI or an installed ORAS binary.
Packing and pushing are not implemented yet.

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

This test requires ORAS 1.3.2. It pushes generated directory and binary-file
fixtures, pulls each with the library and ORAS, and compares paths, contents,
ordinary modes, and symlink targets. The default endpoint is `registry.localhost`;
override it with `OCI_ARTIFACT_TEST_REGISTRY` when using a different registry host.
Each run uses a unique `oci-artifact-tests/` repository prefix and prints the
references and manifest digests. Registry fixtures remain available for inspection;
local fixture and output directories are removed after the test.

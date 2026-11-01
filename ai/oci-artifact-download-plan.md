# Standalone OCI directory downloader

## Outcome and scope

Create an independent Rust library package at `oci-artifact/` in this project.
Include all required registry transport helpers directly in the package. Builds
MUST NOT require another repository, a sibling checkout, or private dependencies.
Diamant will import the package in a later task; no Diamant integration is part
of this implementation.

The target workflow is:

```sh
oras push <reference> payload
```

`payload/` can contain files, nested directories, empty entries, and executable
files. ORAS packs the whole directory into a gzip/tar layer. Download that layer
and use **ocirender** to reconstruct directories and files with their archived
ordinary permissions. Mixed top-level pushes of raw files alongside directories
are outside the initial scope.

Use ocirender's streaming API directly. Do not create an intermediate OCI image
layout, write `index.json` / `oci-layout` files, or require an installed ORAS CLI
at runtime. Temporary downloaded layer blobs are still required by the selected
ocirender API; this is not network-to-extractor streaming without temporary disk
storage.

## Selected approach and accepted differences

Use `oci-client` for registry access and `ocirender` for decompression, layer
processing, and directory output. Rely on its directory sink for extraction
and permission restoration.

The user explicitly accepted the uncommon filename differences as insufficient
reason to implement extraction ourselves. In particular, ocirender treats
`.wh.*` entries as container whiteouts; a literal `.wh.literal` file does not
round-trip like it does through `oras pull`. Document this limitation. Do not
fork ocirender, rewrite whiteout handling, or make literal whiteout filename
preservation a release requirement.

The supported directory workflow is validated against `oras pull`, but this
package does not claim full ORAS artifact compatibility. Ordinary archives
without unpack annotations, raw-file artifacts, and ORAS graph-copy features
are outside the initial directory reconstruction path.

## Verified ocirender behavior

Tests used ORAS 1.3.2. A single-directory fixture contained nested folders,
a 1 MiB binary file, empty files/directories, spaces and Unicode names,
permission modes 0751/0750/0640, and a relative symlink.

`ocirender` 0.3.0 restored this fixture exactly: paths, contents, ordinary modes,
and link target. Directory conversion was tested from a local OCI layout.
Use its streaming interface to avoid that intermediate layout in the
implementation. Files inside the pushed directory are tar members, not
separate raw-file layers.

Prototype reports and sources are under `/private/tmp/oras-directory-probe/`
and `/private/tmp/oras-rust-probe/`. These are temporary compatibility probes.
Implementation tests MUST recreate fixtures and MUST NOT depend on these paths
or expiring ttl.sh references.

## Package structure and reusable transport functionality

Use a library crate named `oci-artifact` (`oci_artifact` in Rust).
Keep it independent of `ws-proxy` and Diamant. A thin CLI or example can exercise
the library without requiring Diamant.

Bring the following reusable behavior into package-local modules:

| Module | Functionality to include | Required adaptation |
| --- | --- | --- |
| `client` | Construct an `oci_client::ClientConfig`: HTTPS by default, explicit plain HTTP and skip-certificate-verification options, PEM CA file loading, HTTP/HTTPS proxy and no-proxy settings, selectable Rust/native TLS features. | Accept typed configuration rather than CLI state. Propagate CA read and client construction errors. Filter empty proxy values. |
| `auth` | Explicit bearer-token or username/password authentication; optional Docker credential lookup using the normalized registry address; translate username/password and identity-token credentials into `RegistryAuth`. | Explicit credentials take precedence. Keep Docker lookup opt-in for library callers and expose lookup diagnostics without logging secrets. Anonymous access remains available. |
| `download` | Parse registry references, pull image manifests, and obtain blob streams with `pull_blob_stream`. Use `backon` exponential backoff for transient manifest/blob transfer failures. | Save each layer separately. Retry with a fresh temporary file after partial failures. Add descriptor size/digest verification and bounded concurrency. Add index/platform resolution using the selected oci-client version; do not assume it is already implemented. |
| `progress` | Count successfully written bytes with an `AsyncWrite` wrapper, including correct handling of partial writes, flush, and shutdown. | Emit per-layer and overall progress events. Keep any Indicatif presentation in the optional CLI/example. Count retries without inflating completed-byte totals. |
| `destination` | Resolve output paths, create missing parents, and open writable files. | Create parents from the actual requested path before opening it. Replace direct truncation of the final output with private blob files, staging, and publication. Do not assign executable permissions to every file. |
| `error` | Typed reference parsing, client configuration, registry, and local I/O failures. | Add verification, rendering, cancellation, and publication errors while preserving underlying causes. |

The directory-output path MUST NOT concatenate layer streams. Each layer remains
a separate verified blob until ocirender has finished reading it. A raw-output
mode, terminal UI, environment-variable parsing, and the full standalone CLI are
not prerequisites for this package.

Preserve applicable third-party notices. Keep Clap, logging setup,
environment parsing, and Indicatif out of the library core. The library MUST NOT
initialize a Tokio runtime, initialize logging, print progress, or exit the
process. Supply credentials/configuration explicitly without process-global
mutation.

Expose a downloader, request/options, platform, destination, cancellation,
progress events, typed errors, and a result containing the resolved manifest
digest and output path. Do not add Diamant-specific settings or lifecycle rules.

## Direct ocirender integration

Source inspection of ocirender 0.3.0 confirms the following interface. Compile
and exercise it as the first integration check; the earlier runtime tests used
`convert_dir`, not `StreamingPacker`.

1. Resolve the reference and select an image manifest. Resolve tags once and
   retain the immutable manifest digest. Support index selection by an explicit
   platform or a documented host-platform default. Reject unavailable or
   ambiguous selections clearly.
2. Validate that the artifact matches the supported directory shape: one named,
   unpackable directory layer plus unnamed manifest/config metadata. Do not
   require the empty config to parse as a container runtime configuration.
3. Build `Vec<ocirender::LayerMeta>` from layer indexes and media types, in
   manifest order. Keep descriptors/digests in our downloader.
4. Create a private output staging directory and construct
   `StreamingPacker::new(metas, ImageSpec::Dir { path: staging }, progress_tx)`.
5. Download each layer to its own temporary file. Stream without buffering the
   complete blob in memory. Verify descriptor size and digest before supplying
   the file to ocirender. Close/flush the writer first.
6. Call `notify_layer_ready(index, blob_path).await` for each completed blob.
   Keep files available until rendering finishes. The API accepts out-of-order
   completion and preserves layer processing order internally. The initial
   single-directory case normally has only one layer.
7. On download failure, signal `notify_error`, stop outstanding downloads, and
   await `finish`. On success, call `finish().await` and require success before
   publishing. Verify cancellation and task shutdown; dropping a packer alone
   is not a substitute for waiting for rendering to stop.
8. Publish the staged tree, then remove temporary blobs. On error or
   cancellation, clean up staging only after renderer tasks have stopped.

Archive paths already include the directory prefix, as verified in the fixture.
Let ocirender unpack into the staging root; do not add a second `payload/`
prefix, strip paths ourselves, or rebuild a filesystem from annotations.
Manifest/config metadata MUST NOT become output files.

## Permissions, errors, and publication

Rely on ocirender's directory sink for tar extraction and permission restoration.
Verify archived ordinary file and directory modes, including executable bits,
on supported platforms. Do not infer permissions from file extensions or apply
a blanket executable mode to every extracted file.

Document ownership, privileged modes, extended metadata, and platform limits
according to verified upstream behavior. Do not promise complete OCI metadata
fidelity. Check path/link confinement through the actual streaming output path;
do not implement a parallel extraction engine to reproduce upstream behavior.

Retry transient registry failures per blob with bounded backoff. Preserve
authentication, verification, local I/O, and rendering errors as distinct causes.
Do not retry malformed archives or local output failures as network failures.

Start with a destination that does not exist. Build staging on the destination
filesystem and publish with a rename that cannot silently replace a destination
created concurrently. Existing-directory merges and overwrites are deferred.
No failed operation may leave partial files looking like a completed pull.
ocirender can leave partial directory output on failure; our staging cleanup
owns that case.

## Implementation progress

The library now supports standalone files as well as directories. This checklist
records implemented behavior; remaining items from the original design are not
implied complete by successful directory round trips.

- [x] Create the independent `oci-artifact` library and explicit transport/authentication options.
- [x] Download and verify one named file or unpackable directory layer.
- [x] Restore directory contents and ordinary permissions through `StreamingPacker`.
- [x] Add bounded transient download retries and caller-driven cancellation.
- [x] Package and upload prepared filesystem content using the ORAS convention.
- [x] Use chunked uploads, with an empty-file fallback and single-pass tar hashing.
- [x] Compare both clients' pushes and pulls against Compose Distribution with ORAS 1.3.2.
- [x] Integrate reference-driven download/report into Diamant with private host staging.
- [ ] Add index/platform selection.
- [ ] Add the planned progress API and optional Docker credential lookup.
- [ ] Add atomic no-replace publication to the library's own download API.
- [ ] Complete the original cross-platform validation matrix.

The library restores directly into its requested destination and can leave partial
updates on extraction failure. Diamant supplies private staging and publishes only
a successful download. HTTP/Git source resolution, in-memory source APIs, OCI
registry relay, and HUB publication integration remain separate work.

## Implementation sequence

1. Create the standalone package and include the transport functionality listed
   above as package-local modules. Separate library configuration/progress/errors
   from CLI presentation and verify that builds need only this repository.
2. Compile a minimal `StreamingPacker` directory-output adapter against the
   selected ocirender version. Verify output and permissions without layout
   metadata files. Pin the tested dependency combination in the lockfile.
3. Connect reference/platform resolution and verified per-layer downloads to
   that adapter. Retain bounded retries and configurable download concurrency.
4. Add cancellation, staging, error propagation, cleanup, and publication.
5. Add a thin CLI or example and document the plain directory push workflow,
   permission behavior, temporary disk use, and accepted filename differences.
6. Run compatibility and failure tests. Present the completed standalone
   package for review. Import it into Diamant only in a later task.

## Validation and completion

Use deterministic fixtures and a disposable local registry. Pin the ORAS
version used for comparison. Public ttl.sh pushes are optional smoke tests
containing only generated public data.

- Compare `oras pull` with our download-to-streaming-render pipeline for a plain
  single-directory push. Cover nested paths, binary/empty files, spaces and
  Unicode names, empty directories, relative symlinks, and archived modes.
- Assert that no OCI layout metadata or reconstructed archive is created as an
  intermediate output. Temporary individual compressed blobs are expected.
- Exercise platform selection and acceptance of ORAS empty-config metadata.
  Reject unsupported mixed/raw artifacts clearly.
- Record `.wh.*` behavior as an accepted upstream difference. Do not require
  ORAS-equivalent literal whiteout-name preservation or add a workaround.
- Test wrong blob sizes/digests, interrupted transfers, retries, malformed
  archives, rendering failure, path/link escapes, cancellation, existing
  destinations, concurrent destination creation, and cleanup.
- Verify progress delivery and renderer shutdown without leaked tasks or
  published partial trees. Avoid tests that merely mirror implementation.
- Run formatting, relevant Rust tests, Clippy, and supported TLS builds. Verify
  Linux and macOS; check Windows dependency compatibility before promising it.

Follow repository approval rules before committing. No commit is part of this
planning update.

## References

- [ocirender streaming interface](https://docs.rs/ocirender/0.3.0/ocirender/struct.StreamingPacker.html)
- [ocirender output formats](https://docs.rs/ocirender/0.3.0/ocirender/enum.ImageSpec.html)
- [Rust OCI client](https://github.com/oras-project/rust-oci-client)
- [ORAS filesystem store](https://github.com/oras-project/oras-go/blob/v2.7.0/content/file/file.go)

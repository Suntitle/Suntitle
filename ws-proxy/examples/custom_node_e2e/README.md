# Local extension fixtures

`just custom-node-e2e` verifies six extensions through the real control
API, publication controller, Distribution through Traefik, and Diamant:

| Source directory | Runtime | Entry point |
| --- | --- | --- |
| `extensions/native` | `executable` | `./bin/probe` |
| `extensions/python-script` | `python` | `./probe.py` |
| `extensions/python-module` | `pythonModule` | `probe_module` |

Each payload runs once from a local source and once from an OCI source. For the
OCI cases, `oci_artifact::Publisher::republish` pushes the already published local
payload into a separate `custom-node-sources` repository. The `oci-*-extension.yaml`
requests reference those source tags. HUB mirrors them into its managed repository;
the test checks that repository changes while the manifest digest is preserved.
No external registry, ORAS, or container-image CLI is required.

Generate certificates with `just generate-certs` first. The test requires Docker
Compose with `!reset`/`!override` support, Rust, Unix, `/usr/bin/python3`, and free
ports 3030, 3031, 16443, and 18080. It runs the API, proxy, and Diamant locally and
starts isolated Kubernetes storage, Distribution, and Traefik in a separate Compose
project. Temporary kubeconfigs point to port 16443. The test uses an isolated host
artifact directory. Test infrastructure and its volumes are removed on exit.

The `*-extension.yaml` and `*-node.yaml` files are the complete API request bodies.
`scenarios.yaml` enumerates cases; `expectations.yaml` contains expected process
arguments. The harness replaces `${SOURCE_ROOT}`, `${HOST_NAME}`, and `${RUN_ROOT}`
in string values. `${REGISTRY}` selects the registry authority; `${RUNTIME}`
identifies the expected report filename. No shell expansion occurs. The message
intentionally contains spaces and shell syntax.

Each case waits for current-generation publication and checks the digest reference.
A node watch starts at the creation resource version and MUST observe
`Installed=True`, `Pending`, and `Ready=False/AwaitingLaunch` before
`Running/Ready=True`. Process JSON reports confirm exact arguments, defaults,
overrides, and execution from downloaded files. Python runs through embedded UV
with a pinned local interpreter and downloads disabled. Fixtures have no external
dependencies. This covers launch, not the audio/text WebSocket protocol. Fixtures
exit on a finish file or after 120 seconds.

## Development use

Sources remain after the test. `just custom-node-fixtures` rebuilds the ignored
`extensions/native/bin/probe` executable, which also survives E2E cleanup. Rebuild
it when the target host OS or architecture changes. It MUST match the Diamant host,
rather than the HUB container.

Normal Compose mounts `extensions` read-only into ws-proxy at
`/opt/suntitle/example-extensions`. Developers' own sources use the separate
repository-root `extensions/` directory, mounted at `/opt/suntitle/extensions`.
Set `LOCAL_EXTENSIONS_DIR` in `.env.local` to select another source directory.
For requests using these fixtures, substitute:

- `${SOURCE_ROOT}`: `/opt/suntitle/example-extensions` for Compose HUB, or the absolute
  repository fixture `extensions` directory for a locally running HUB.
- `${HOST_NAME}`: the registered target host's resource name.
- `${RUN_ROOT}`: an existing writable directory on the Diamant host for reports
  and the optional `finish` file. Use a fresh directory without a `finish` file.

Convert the resulting YAML bodies to JSON and POST the extension to
`/api/v1/extensions`, then the node to `/api/v1/hosts/{host-uid}/nodes`.
Wait for publication before creating the node. Use `--artifact-plain-http` with
Diamant for the local Compose registry. For Python fixtures, configure a suitable
local interpreter via `UV_PYTHON` if needed.

The E2E registry is `127.0.0.1:18080`; development uses the normal Compose registry,
`registry.localhost`. That hostname MUST resolve and be reachable from Diamant.

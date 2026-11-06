# Suntitle

Suntitle streams microphone audio through a WebSocket proxy, transcribes it with
Whisper, optionally translates the text, and displays the result in an
always-on-top subtitle window.

```text
microphone -> ws-proxy -> Whisper transcriber -> translator -> subtitle window
```

The proxy can run locally or on an AWS EC2 instance created by Pulumi. The
audio, transcription, translation, and display processes run locally in both
setups.

## Prerequisites

- Python 3.10 and [uv](https://docs.astral.sh/uv/) for the standalone Python commands. Diamant embeds uv and does not need a uv executable.
- Rust and Cargo
- [just](https://just.systems/) command runner
- A working microphone/input device
- PortAudio, if it is not already available on the machine (`brew install
  portaudio` on macOS)

The first transcriber and translator runs download their models from Hugging
Face, so they require internet access and can take a while to start.

Install the Python dependencies from the repository root:

```bash
just setup
```

## Local stack

Docker Compose runs PostgreSQL, Kine, kube-apiserver, the control API, and the
WebSocket proxy/controller, Distribution OCI registry, and the UI development server.
Diamant runs on the host.

```bash
just generate-certs  # once before the first launch; requires mkcert and OpenSSL
just local-stack
```

`just local-stack` creates `.env.local` if missing, generates local client
kubeconfigs, builds the backend and UI images, and starts Compose. A bootstrap container
waits for Kubernetes readiness and installs the namespace, CRDs, and RBAC.
The API and proxy start with their separate container kubeconfigs. The command
waits for the API, proxy, and UI to be healthy before starting Diamant.
PostgreSQL is Kine's backing database; legacy application database migrations
are not needed.

Open http://localhost. Traefik routes `/api` to the control API and
`/register`, `/transcribe`, `/translate`, and `/display` to the WebSocket proxy.
WebSocket audio connections at `/` also reach the proxy; normal page requests
and Vite hot-reload connections reach the UI. Diamant registers through
`ws://localhost/register`.
Only Traefik publishes a host port (80); PostgreSQL, Kine, and Kubernetes
are accessible inside Compose only.
The API stores resources through Kubernetes. Nodes installed through the UI
are reconciled asynchronously by the controller and executed by Diamant.

Ctrl+C or a startup failure stops Diamant and its node processes, stops the UI,
and removes the Compose containers and network. The database volume is retained.
`docker compose --env-file .env.local stop` stops the infrastructure while
preserving its data volume. To erase the local database and all Kubernetes
resources, stop `just local-stack`, then run `just db-drop-volume`. The next
`just local-stack` creates a fresh database and bootstraps Kubernetes again.

After generating certificates and kubeconfigs, the container stack can also run on its own:

```bash
docker compose --env-file .env.local build kubernetes-bootstrap ws-proxy ui
docker compose --env-file .env.local up -d --wait
```

Standalone clients can use `localhost:80` instead of `localhost:3030` for
the WebSocket proxy, and `http://localhost/api` for the control API.
Commands that access Kubernetes on host port 6443 require separate connectivity;
that port is no longer published by the stack.

### Extension artifact registry

For your own local extensions, put each payload in `extensions/<name>/` and register
it with `sourceType: local` and `source: /opt/suntitle/extensions/<name>`.
Compose mounts that directory read-only into HUB. Set `LOCAL_EXTENSIONS_DIR` in
`.env.local` to use another host directory. Its contents are ignored by Git.
The bundled E2E fixtures are available separately at
`/opt/suntitle/example-extensions/{native,python-script,python-module}`.

Distribution 3 stores OCI artifacts in the persistent `registry-data` volume,
independently of Kubernetes. Traefik routes `registry.localhost` to its private
port 5000. Set `REGISTRY_HOST` in `.env.local` to override the hostname. Clients
MUST use that reachable authority in artifact references, for example
`registry.localhost/suntitle/extensions/example:smoke`.

Traefik depends on the registry, API, proxy, and UI being healthy. Starting it
also starts those services and their dependencies. After preparing certificates
and building the images as above, check the OCI API:

```bash
docker compose --env-file .env.local up -d --wait
curl --resolve registry.localhost:80:127.0.0.1 -fsS http://registry.localhost/v2/
```

With ORAS installed, verify artifact push and pull through Traefik:

```bash
(
  cd "$(mktemp -d)"
  printf 'registry smoke test\n' > smoke.txt
  oras push --plain-http registry.localhost/suntitle/extensions/example:smoke \
    smoke.txt:text/plain
  oras pull --plain-http registry.localhost/suntitle/extensions/example:smoke \
    --output pulled
  cmp smoke.txt pulled/smoke.txt
)
```

The local endpoint uses HTTP without authentication. Deployed environments MUST
configure HTTPS and authentication at Traefik or Distribution, use a hostname
reachable by assigned Diamant hosts, and pin a tested registry patch version or
image digest. Changing `REGISTRY_HOST` alone does not enable TLS or authentication.
Back up `registry-data`; container replacement and ordinary Compose shutdown
preserve it. `docker compose down --volumes` (including `just db-drop-volume`)
also erases registry artifacts.

For a local backup, capture a PostgreSQL custom-format dump:

```bash
docker compose exec -T postgres pg_dump -U suntitle -d suntitle -Fc > suntitle.dump
```

To restore into a separate empty database (keeping the current database intact):

```bash
docker compose exec -T postgres createdb -U suntitle suntitle_restore
docker compose exec -T postgres pg_restore -U suntitle -d suntitle_restore < suntitle.dump
```

Set `POSTGRES_DB=suntitle_restore` in `.env.local`, then recreate Kine before
starting the applications.

### Standalone Kubernetes API server

Kine stores Kubernetes state in the existing PostgreSQL database. To start the
API server, first generate the TLS, client CA, and service-account signing files
in `pki/` using the local certificate script (requires `mkcert` and OpenSSL):

```bash
just generate-certs
docker compose --env-file .env.local up -d kube-apiserver
just kubectl get --raw=/readyz
```

`just kubectl` forwards all arguments to kubectl, targeting `https://127.0.0.1:6443`
with the generated admin certificates. It does not use or modify your default
kubeconfig. For example:

```bash
just kubectl get namespaces
just kubectl get pods -A
just kubectl apply -f manifest.yaml
```

Compose starts PostgreSQL, waits for its health check, then starts Kine and the
API server. Kine exposes port `2379`; the HTTPS Kubernetes API exposes `6443`.
The `pki/` directory is mounted read-only and excluded from Git. This standalone
setup does not run a controller-manager, scheduler, or worker nodes.

Compose defaults match the existing local database credentials. It reads
`.env.local` only when passed `--env-file .env.local`. PostgreSQL initialization variables do not change
credentials in an existing data volume. If you override `APISERVER_ADVERTISE_ADDRESS`
or `SERVICE_CIDR`, ensure the server certificate covers the address clients use
and the Kubernetes service IP in that range.

Stop the Kubernetes services while retaining PostgreSQL and its data:

```bash
docker compose stop kube-apiserver kine
```

## Run the web UI with the API backend

After generating certificates, start Kubernetes, the control API, and UI from
the repository root. Press `Ctrl+C` to stop the API and UI; Kubernetes stays available:

```bash
just ui
```

See [`ui/README.md`](ui/README.md) for separate service commands, custom API
targets, and Cypress testing.

## Run the complete stack locally

Start each process in a separate terminal, from the repository root. Start the
consumers before the microphone because the proxy does not buffer messages for
clients that are not connected.

1. Prepare Kubernetes with `just kubernetes-up`, then start the
   WebSocket proxy on port `3030`. Start `just api` in another terminal for the
   independent control API on port `3031`:

   ```bash
   just proxy-release
   ```

2. Start the subtitle window:

   ```bash
   just subtitle
   ```

3. Start the translator:

   ```bash
   just translator
   ```

   The current translator is fixed to Ukrainian-to-Czech translation using
   `Helsinki-NLP/opus-mt-uk-cs`.

4. Start the Whisper transcriber and wait for it to finish loading the model:

   ```bash
   just transcriber
   ```

   `small` is a practical local default. Use another supported Whisper model if
   desired; run `just transcriber-help` for all options.

5. Stream microphone audio to the proxy's root WebSocket endpoint:

   ```bash
   just listener
   ```

   To inspect available input devices and then select one by index or a unique
   part of its name:

   ```bash
   just listener-devices
   just listener-device 2
   ```

Press `Ctrl+C` in the terminal processes to stop them. Press `Esc` while the
subtitle window is focused to close it.

If only raw transcription is needed, the translator and subtitle window may be
omitted; completed text is printed by `whisper_online_client.py`.

## Manage hosts and nodes through the API

The proxy and control API share durable Kubernetes host and node resources. The following
quickstart runs both services plus one Diamant host and installs an audio-source
node on it. Run each service command from the repository root in a separate
terminal.

1. Run `just kubernetes-up`, then start `just proxy` and `just api`
   in separate terminals.

2. Start Diamant. Its host name resolves to a stable Kubernetes resource UID.
   `--project-dir` tells it where to launch node programs:

   ```bash
   just diamant-local
   ```

3. List hosts and copy `metadata.uid` and `metadata.name` of `local-host`:

   ```bash
   just hosts
   ```

4. Install and start an audio-source node on that host:

   ```bash
   just install-audio-source <host-uid> <host-name>
   ```

   Diamant launches the audio listener immediately. Set `deviceName` to a device
   name when the system default is not the intended input.

5. List the nodes, then use the created node's `metadata.name` to inspect it:

   ```bash
   just nodes
   just node <node-name>
   ```

Use `PUT /api/v1/nodes/{name}` with `{"spec": ...}` containing the complete
editable spec to change its name, enabled state, settings, or input. Controllers
reconcile desired state and dispatch runtime operations to Diamant. Disconnecting
Diamant marks the host offline and retains desired nodes in Kubernetes.

## Quick local end-to-end test

The repository includes a lightweight WebSocket probe that verifies the entire
proxy relay without loading Whisper, the translation model, the microphone, or
the subtitle UI. It connects mock clients to every route and checks this flow:

```text
mock audio -> /transcribe -> /translate -> /display
```

Start the proxy from the repository root:

```bash
just proxy-release
```

In a second terminal, run the probe:

```bash
just e2e
```

A successful run prints:

```text
PASS: live WebSocket relay delivered audio -> transcription -> translation -> display
```

The probe disconnects its mock clients automatically. Return to the proxy
terminal and press `Ctrl+C` to shut down the server.

To verify the complete host-registration path, run the self-contained host E2E
example. Generate certificates first with `just generate-certs`. The command
starts Kubernetes, builds and starts `ws-proxy` and `control_api`,
starts a temporary `diamant` agent,
and checks that the host is returned by the control API:

```bash
just host-e2e
```

The example stops its application children and removes its isolated Kubernetes
namespace when it finishes. Kubernetes remains running.
Both E2Es require ports 3030 and 3031 to be free.
The recipes depend on `just e2e-build` to prepare applications and examples before
startup. Direct example execution requires this preparation too; harnesses share
process guards, service waits, and API polling under `ws-proxy/examples/support`.

To verify node lifecycle handling end to end, run:

```bash
just node-e2e
```

This starts the proxy/API and Diamant locally, registers audio-source and display
nodes, verifies listing and concurrent launches, and checks service restart
replay. Real embedded uv runs a dependency-free Python fixture, so the test
does not use audio hardware, models, or a graphical session. Fixtures exit
themselves before Diamant disconnects; this test does not cover child
cancellation. The check currently requires Unix and `/usr/bin/python3`.

To verify local and OCI extension registration through actual native, Python
script, and Python module execution, run `just custom-node-e2e`. The YAML requests
and retained development sources are in
[`ws-proxy/examples/custom_node_e2e`](ws-proxy/examples/custom_node_e2e/README.md).
Compose mounts those fixtures into HUB at `/opt/suntitle/example-extensions`.
The custom-node recipe explicitly depends on `custom-node-fixtures`, which builds
the E2E binaries and copies the native probe into its retained source directory.

## Run the Compose stack on AWS with Pulumi

The Pulumi setup in `infrastructure/` installs Docker on EC2 and runs the same
Compose stack used locally. Diamant and audio processing run on client hosts.

Open `http://<public-ip>` after cloud-init finishes. The gateway exposes port 80;
backing services stay private. See [AWS setup](infrastructure/README.md) for
deployment commands, credentials, logs, persistent data, and destruction instructions.

## WebSocket routes

| Route | Client | Data flow |
| --- | --- | --- |
| `/` | `suntitle-listener.py` | microphone audio into the proxy |
| `/transcribe` | `whisper_online_client.py` | audio in, transcription text out |
| `/translate` | `suntitle-translator.py` | transcription in, translated text out |
| `/display` | `suntitle.py` | translated text into the subtitle window |
| `/register` | `diamant` | host registration and presence connection |

## Run the Diamant host agent

`diamant` is built from the same Rust package as `ws-proxy`. Default builds run
the headless host agent and do not require Qt at build time or runtime.

For headless operation, run one CLI instance on each host that should register
with the proxy:

```bash
diamant
```

The host name defaults to the operating system hostname and can be overridden
with `--host-name` or `SUNTITLE_HOST_NAME`. The proxy URL can be set with
`--proxy-url` or `SUNTITLE_PROXY_URL`. Diamant detects the routed host IP by
default; use `--host-ip` or `SUNTITLE_HOST_IP` to override it.

The proxy resolves the host name to a Kubernetes host resource and returns its
UID. Diamant reports health every 30 seconds and reconnects when the connection
ends. The health interval can be changed with `--health-interval-seconds` or
`SUNTITLE_HEALTH_INTERVAL_SECONDS`.

To enable the optional desktop window, build with the `gui` Cargo feature:

```bash
cargo build -p diamant --bin diamant --features gui
```

GUI builds open the desktop window by default; `diamant --cli` runs headless.
The window stores connection settings locally, connects to the hub on demand,
and shows the nodes running on the host with their command parameters.
The desktop fields map to the same CLI options. `diamant uv ...` remains
available in both builds for invoking the embedded uv CLI. The `--cli` flag
also remains accepted in default builds.

Building with `gui` requires the Qt 6 base and declarative development packages.
GUI binaries require the corresponding Qt 6 runtime libraries, even in CLI mode.
CI release builds use the default features and do not require Qt.

Host identity, health, desired nodes, and dispatch status persist in Kubernetes resources.
Disconnected hosts become offline; missing health reports expire after 45 seconds,
so configure the health interval below that limit. Nodes remain available through
the API while the host is offline. CI publishes
Diamant as `diamant.<target>.tar.xz` alongside the proxy release.

## Presubmit checks

Pull requests and merge queue entries run these GitHub Actions checks:

All tests run through `just`: `test-rust`, `test-rust-live`, `kubernetes-test`,
`host-e2e`, `node-e2e`, `e2e`, `test-python`, and `ui-test`.

- `Rust and Kubernetes E2E`: Rust tests, live Kubernetes integration tests,
  resource contract and RBAC checks, and the `host_registration_e2e`,
  `node_handling_e2e`, and `e2e_probe` examples.
- `Python mock API and node CLI tests`: the Python unittest suite.
- `Lint, build, and test UI`: UI lint, production build, and Cypress tests.

The backend job starts PostgreSQL, Kine, and the Kubernetes API with temporary
certificates. Lifecycle examples run sequentially because they use the same
local ports. `fake_uv` supports the node lifecycle example; it is not a separate
test. These checks use simulated node processes and relay payloads, without
downloading speech or translation models. Cypress tests use intercepted API
responses.

Backend service logs and Cypress screenshots/videos are uploaded on failure.
The backend workflow also supports manual runs. To enforce these checks before
merging, select the three check names above in the repository's branch rules.

### Diamant artifact downloads

Once an extension has a published artifact reference for its current generation,
HUB resolves it into `SuntitleNode.status.artifactReference`. Diamant receives a
`download_artifact` command and downloads from that reference using `oci-artifact`.
The reference contains the registry and repository; the client supplies the standard
OCI `/v2/` API path. No registry suffix needs configuration. Published digest
references are preferred so the downloaded content is immutable.

Downloads use anonymous access and verified HTTPS by default. For the local
Compose registry, run Diamant with `--artifact-plain-http` or
`SUNTITLE_ARTIFACT_PLAIN_HTTP=true`. Artifact storage is configured with
`--artifact-dir` or `SUNTITLE_ARTIFACT_DIR`. The default is the OS local data directory:

- Linux: `$XDG_DATA_HOME/suntitle/diamant/artifacts`, normally
  `~/.local/share/suntitle/diamant/artifacts`.
- macOS: `~/Library/Application Support/suntitle/diamant/artifacts`.

Each operation downloads into a private staging directory and publishes a unique
payload directory only after success. Download reports include its absolute path.
Failed or cancelled downloads clean up staging. Completed payloads are retained;
automatic cache reuse and garbage collection are deferred. Reconnecting downloads
the current artifact again. Storage is independent of the Python project directory.

Downloads run in the background so heartbeats continue. Removal, replacement,
disconnect, and shutdown cancel outstanding work and await renderer cleanup.
Reports are checked against node, generation, operation, artifact, and host session.

This stage downloads and reports only. Success leaves the node `Pending` with
`Installed=False`, `Ready=False`, and `Updated=False`, reason `ArtifactDownloaded`.
Failure sets `Failed`, reason `ArtifactDownloadFailed`. Prerequisite preparation
and custom execution are deferred. Existing agents must be upgraded to understand
the new registration command before extension downloads can be dispatched to them.

Git extension sources use HTTP or HTTPS through `gix` and rustls. Each preparation
fetches default `HEAD` at depth one into a private temporary bare repository, then
streams that commit's files into the extension artifact without a working checkout.
The temporary repository is removed when packaging finishes or fails. Cancellation
interrupts fetching and waits for cleanup. SSH, Git submodules, and Git LFS content
resolution are deferred. The application does not invoke external Git or SSH tools.

## Rust workspace

The repository root contains the Cargo workspace and its shared `Cargo.lock`.
Build output is written to `target/` at the root.

| Package | Location | Responsibility |
| --- | --- | --- |
| `ws-proxy` | `ws-proxy/` | WebSocket relay and reconciliation controller |
| `control-api` | `control-api/` | HTTP control API, Kubernetes storage, and OpenAPI generation |
| `diamant` | `diamant/` | Host agent, extension installation, and node processes |
| `diamant-ui` | `diamant-ui/` | Optional Qt desktop UI for Diamant |
| `oci-artifact` | `oci-artifact/` | OCI artifact publishing and downloading |
| `suntitle-core` | `suntitle-core/` | Shared models, Kubernetes resources, and registration protocol |

The proxy, API, and agent depend on `suntitle-core`; they do not depend on one
another at runtime. Proxy integration tests use `control-api` as a development
dependency. End-to-end examples remain in `ws-proxy/examples/` and use the
workspace binaries.

```sh
cargo build --locked
cargo test --locked
cargo run -p control-api --bin control_api -- --help
cargo run -p control-api --bin generate_openapi
cargo run -p diamant -- --help
cargo check -p diamant --features gui
```

Default workspace commands omit the Qt-only `diamant-ui` package. To build or
test all headless packages explicitly, use `--workspace --exclude diamant-ui`.
The optional GUI requires Qt development libraries.

Headless Linux releases use static `x86_64-unknown-linux-musl` binaries for
`ws-proxy`, `control_api`, and `diamant`. Build and verify them locally with:

```sh
docker build --platform linux/amd64 --file docker/ws-proxy/Dockerfile \
  --target release-artifacts --output type=local,dest=target/static-release .
```

The build rejects shared libraries and dynamic loaders, then runs each binary's
`--help`. The optional Qt GUI is excluded from these release artifacts.

The release workflow publishes a separate image for each component from the same
compiled binaries used in release archives:

| Component | Image | Default executable |
| --- | --- | --- |
| Control API | `ghcr.io/suntitle/suntitle/control-api:<release-tag>` | `control_api` |
| WebSocket proxy | `ghcr.io/suntitle/suntitle/ws-proxy:<release-tag>` | `ws-proxy` |
| Headless Diamant | `ghcr.io/suntitle/suntitle/diamant:<release-tag>` | `diamant` |

Stable releases update each image's `latest` tag; prereleases publish only their
version tag. Each image contains its component's executable. The API also serves
Kubernetes bootstrap commands. Local Compose builds the `control-api` and
`ws-proxy` Docker targets independently.

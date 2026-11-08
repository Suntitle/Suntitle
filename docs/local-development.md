# Local development

Start with the [root quickstart](../README.md#quickstart). All commands below run
from the repository root. Run `just --list` to see available recipes.

## Python and audio prerequisites

Standalone Python commands need Python **3.11 or newer** and
[uv](https://docs.astral.sh/uv/). Diamant embeds uv; its launcher does not require
a separate uv executable. Audio capture needs a microphone and PortAudio
(`brew install portaudio` on macOS).

```bash
just setup
```

The first transcriber and translator runs download models from Hugging Face.
They require internet access and may take a while to start.

## Compose stack

Docker Compose runs PostgreSQL, Kine, kube-apiserver, the control API, and the
WebSocket proxy/controller, Distribution OCI registry, and the UI development server.
Diamant runs on the host.

```bash
just db-env  # set ADMIN_USERNAME and ADMIN_PASSWORD in .env.local
just generate-certs  # once before the first launch; requires mkcert and OpenSSL
just local-stack
```

Install ORAS 1.3 or newer on the host before starting the stack.
`just local-stack` creates `.env.local` if missing and generates local client
kubeconfigs. It starts Distribution and Traefik, publishes the complete
`default-extensions/` checkout with ORAS, and pins the proxy to the local artifact's
digest before building and starting the backend and UI images. A bootstrap container
waits for Kubernetes readiness and installs the namespace, CRDs, and RBAC.
The API and proxy start with their separate container kubeconfigs. The command
waits for the API, proxy, and UI to be healthy before starting Diamant.
PostgreSQL stores Kine's state and the administrator account. The API creates
its administrator table automatically; legacy application migrations are not needed.

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
resources and registry artifacts, stop `just local-stack`, then run `just db-drop-volume`. The next
`just local-stack` creates a fresh database and bootstraps Kubernetes again.

Project bind mounts for PKI, registry configuration, and extension sources use
shared SELinux labeling (`z`). Kubernetes, bootstrap, the API, and the proxy share
the PKI directory. Recreate affected containers after changing mount options so
Docker applies the labels. Private keys retain mode `600`.
Traefik alone disables SELinux label confinement to access the host Docker socket
for service discovery. The socket keeps its host label and permissions;
`no-new-privileges` remains enabled.

After generating certificates and kubeconfigs, the container stack can also run on its own:

```bash
just _local-infra-up
```

Standalone clients can use `localhost:80` instead of `localhost:3030` for
the WebSocket proxy, and `http://localhost/api` for the control API.
Commands that access Kubernetes on host port 6443 require separate connectivity;
that port is no longer published by the stack.

## Backup and restore

For a local backup, capture a PostgreSQL custom-format dump:

```bash
docker compose exec -T postgres pg_dump -U suntitle -d suntitle -Fc > suntitle.dump
```

To restore into a separate empty database (keeping the current database intact):

```bash
docker compose exec -T postgres createdb -U suntitle suntitle_restore
docker compose exec -T postgres pg_restore -U suntitle -d suntitle_restore < suntitle.dump
```

Set `POSTGRES_DB=suntitle_restore` in `.env.local`, then recreate Kine and the
API with that configuration before starting the applications.

The PostgreSQL dump does not include OCI payloads. Back up the `registry-data`
volume separately; see [extensions](extensions.md#extension-artifact-registry).

## Host access to Kubernetes and PostgreSQL

Kine stores Kubernetes state in the existing PostgreSQL database. To start the
API server, first generate the TLS, client CA, and service-account signing files
in `pki/` using the local certificate script (requires `mkcert` and OpenSSL):

```bash
export COMPOSE_FILE=compose.yaml:compose.ci.yaml
just generate-certs
just generate-kubeconfigs
docker compose --env-file .env.local up -d --wait kube-apiserver
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
API server. Kine listens internally on port `2379`; the CI overlay publishes the HTTPS
Kubernetes API on loopback port `6443` and PostgreSQL on loopback port `15432`.
The `pki/` directory is mounted read-only and excluded from Git. This standalone
setup does not run a controller-manager, scheduler, or worker nodes.

Set `ADMIN_USERNAME` and `ADMIN_PASSWORD` in `.env.local` before the first
API startup. Set `ADMIN_PASSWORD` to the plaintext password; do not encrypt or
hash it yourself. The API creates a singleton administrator in PostgreSQL and
hashes the password with a random salt using scrypt before storing it. The
plaintext password is not stored in the database. Empty bootstrap credentials prevent startup when
no administrator exists. Login accepts only the persisted administrator's credentials.
Later starts preserve the account and its ID; changing these environment variables
does not reset the password. Removing the database volume removes the account too.
Bearer sessions last 24 hours and are revoked by logout or an API restart.

For an API process running outside Compose, export `DATABASE_URL` pointing to the
same PostgreSQL database, plus the two bootstrap variables on first startup.
PostgreSQL must be reachable from that process; Compose does not publish its port
by default. `generate-crds` and `bootstrap-kubernetes` do not require these variables.
The PostgreSQL bootstrap test runs against a disposable schema:
`TEST_DATABASE_URL=postgres://... cargo test -p control-api database_bootstrap_preserves_credentials -- --ignored`.
The test database user must be allowed to create schemas.

Compose defaults match the existing local database credentials. It reads
`.env.local` only when passed `--env-file .env.local`. PostgreSQL initialization variables do not change
credentials in an existing data volume. If you override `APISERVER_ADVERTISE_ADDRESS`
or `SERVICE_CIDR`, ensure the server certificate covers the address clients use
and the Kubernetes service IP in that range.

Stop the Kubernetes services while retaining PostgreSQL and its data:

```bash
docker compose stop kube-apiserver kine
```

## UI development

For UI work without backend infrastructure, use `just ui-mock`.
For `just ui` with the real API, prepare certificates and use the host-service
environment below first. See [UI development](../ui/README.md) for Vite settings
and Cypress commands.

## Run services on the host

The default Compose stack keeps Kubernetes and PostgreSQL private. Before using
`just kubernetes-up`, `just api`, `just proxy`, or `just ui`, stop the full stack
and select the overlay that exposes those dependencies on loopback:

```bash
export COMPOSE_FILE=compose.yaml:compose.ci.yaml
export DATABASE_URL=postgres://suntitle:suntitle-local@127.0.0.1:15432/suntitle
# Match the persisted administrator, or choose credentials for a fresh database.
export ADMIN_USERNAME=admin
export ADMIN_PASSWORD='<your-password>'
just generate-certs  # only if certificates do not exist
just kubernetes-up
```

Adjust `DATABASE_URL` if your database credentials differ from the defaults.
The host API reads these exported variables; `.env.local` is passed to Compose.
Default host ports are 3030 for the proxy and 3031 for the API. API requests
require a bearer token; see [contracts](contracts.md#http-control-api).

### Standalone audio pipeline

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

For an already running `just local-stack`, skip starting another proxy/API and
point the clients at Traefik instead: `just subtitle localhost:80`,
`just translator localhost:80`, `just transcriber localhost:80`, and
`just listener localhost:80`. Avoid running duplicate nodes for the same stage.

## Verification

```bash
just test-rust
just test-python
just ui-test
```

The commands above cover Rust tests that need no live Kubernetes server, Python
mock API and node CLI tests, and Cypress with intercepted API responses.
For live Rust tests, prepare the host-service environment above and export
`TEST_DATABASE_URL="$DATABASE_URL"`, then run `just test-rust-live`.
`just kubernetes-test` checks resource schemas and RBAC against the live server.

### End-to-end checks

Prepare the host-service environment above before starting the proxy or E2Es.

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
# Publish PostgreSQL and Kubernetes locally for the host-run API.
export COMPOSE_FILE=compose.yaml:compose.ci.yaml
export DATABASE_URL=postgres://suntitle:suntitle-local@127.0.0.1:15432/suntitle
export TEST_DATABASE_URL="$DATABASE_URL"
# Use the persisted administrator credentials if the volume already has an account.
export ADMIN_USERNAME=e2e-admin ADMIN_PASSWORD=e2e-admin-password
just host-e2e
```

The example stops its application children and removes its isolated Kubernetes
namespace when it finishes. Kubernetes remains running.
Both E2Es require ports 3030 and 3031 to be free, and inherit the database
and bootstrap variables shown above. CI sets these variables explicitly.
The recipes depend on `just e2e-build` to prepare applications and examples before
startup. Direct example execution requires this preparation too; harnesses share
process guards, service waits, and API polling under `ws-proxy/examples/support`.

To verify node lifecycle handling end to end, use the same environment and run:

```bash
just node-e2e
```

This starts the proxy/API and Diamant locally, registers audio-source and display
nodes, verifies listing and concurrent launches, and checks service restart
replay. Real embedded uv runs a dependency-free Python fixture, so the test
serves the fixture from a local OCI endpoint and downloads it into Diamant's
artifact cache. It does not use audio hardware, models, or a graphical session. Fixtures exit
themselves before Diamant disconnects; this test does not cover child
cancellation. The check currently requires Unix and `/usr/bin/python3`.

To verify local and OCI extension registration through actual native, Python
script, and Python module execution, run `just custom-node-e2e`. This recipe uses
an isolated database on port 15433 and supplies its own test admin credentials. The YAML requests
and retained development sources are in
[`ws-proxy/examples/custom_node_e2e`](../ws-proxy/examples/custom_node_e2e/README.md).
Compose mounts those fixtures into HUB at `/opt/suntitle/example-extensions`.
The custom-node recipe explicitly depends on `custom-node-fixtures`, which builds
the E2E binaries and copies the native probe into its retained source directory.

## Deployment

See [AWS deployment](../infrastructure/README.md) for running the Compose stack
on EC2 with Pulumi. Diamant and the processing nodes run on client hosts.

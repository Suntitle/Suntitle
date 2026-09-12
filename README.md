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
WebSocket proxy/controller. Diamant and the UI development server run on the host.

```bash
just generate-certs  # once before the first launch; requires mkcert and OpenSSL
just local-stack
```

`just local-stack` creates `.env.local` if missing, generates local client
kubeconfigs, builds the backend image, and starts Compose. A bootstrap container
waits for Kubernetes readiness and installs the namespace, CRDs, and RBAC.
The API and proxy start with their separate container kubeconfigs. The command
waits for both services to be healthy before starting Diamant and the UI.
PostgreSQL is Kine's backing database; legacy application database migrations
are not needed.

Open http://127.0.0.1:5173. The UI forwards `/api` to the control API on port 3031,
which stores resources through Kubernetes on port 6443. Diamant connects to the
proxy on port 3030 and registers `local-host`. Nodes installed through the UI
are reconciled asynchronously by the controller and executed by Diamant.

Ctrl+C or a startup failure stops Diamant and its node processes, stops the UI,
and removes the Compose containers and network. The database volume is retained.
`docker compose --env-file .env.local stop` stops the infrastructure while
preserving its data volume. To erase the local database and all Kubernetes
resources, stop `just local-stack`, then run `just db-drop-volume`. The next
`just local-stack` creates a fresh database and bootstraps Kubernetes again.

After generating certificates and kubeconfigs, the backend can also run on its own:

```bash
docker compose --env-file .env.local build kubernetes-bootstrap
docker compose --env-file .env.local up -d --wait api ws-proxy
```

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

## Run the proxy on AWS with Pulumi

The Pulumi program in `infrastructure/` creates an Amazon Linux EC2 instance and
a security group, then installs the `ws-proxy` binary from the latest GitHub
release. It does **not** deploy the Whisper transcriber, translator, microphone
listener, or subtitle UI.

In addition to the local prerequisites, this setup needs:

- the Pulumi CLI, logged in to a backend;
- AWS credentials able to create EC2 instances and security groups; and
- a GitHub token in `GITHUB_TOKEN` that can read this repository's releases.

The repository's latest GitHub release must contain an asset named exactly:

```text
ws-proxy.x86_64-unknown-linux-gnu.tar.xz
```

The archive must extract a `ws-proxy` executable. Provisioning will not produce
or upload this release asset.

From the repository root, deploy the `dev` stack:

```bash
export GITHUB_TOKEN="<token>"
just infra-select
just infra-preview
just infra-up
```

For a new Pulumi backend where the stack does not exist yet, initialize it with
`just infra-init` instead of selecting it. The checked-in dev configuration
uses AWS region `eu-north-1`; change it before deployment if needed:

```bash
just infra-region <region>
```

Get the proxy address after deployment:

```bash
just infra-ip
```

Cloud-init downloads and starts the release after the instance is created. Wait
until TCP port `3030` accepts connections, then return to the repository root
and run the local components in separate terminals, replacing `<public-ip>`:

```bash
just subtitle <public-ip>:3030
```

```bash
just translator <public-ip>:3030
```

```bash
just transcriber <public-ip>:3030
```

```bash
just listener <public-ip>:3030
```

Port `3030` is currently exposed publicly and uses unencrypted WebSockets
(`ws://`). Do not send sensitive audio over this deployment without first
adding transport security and restricting the security-group ingress rules.
The current Pulumi program also places `GITHUB_TOKEN` in EC2 user data and the
Pulumi state; use a short-lived, read-only token.

Destroy the AWS resources when they are no longer needed:

```bash
export GITHUB_TOKEN="<token>"
just infra-destroy
```

## WebSocket routes

| Route | Client | Data flow |
| --- | --- | --- |
| `/` | `suntitle-listener.py` | microphone audio into the proxy |
| `/transcribe` | `whisper_online_client.py` | audio in, transcription text out |
| `/translate` | `suntitle-translator.py` | transcription in, translated text out |
| `/display` | `suntitle.py` | translated text into the subtitle window |
| `/register` | `diamant` | host registration and presence connection |

## Run the Diamant host agent

`diamant` is built from the same Rust package as `ws-proxy`. Run one instance on
each host that should register with the proxy:

```bash
just diamant
```

The host name defaults to the operating system hostname and can be overridden
with `--host-name` or `SUNTITLE_HOST_NAME`. The proxy URL can be set with
`--proxy-url` or `SUNTITLE_PROXY_URL`. Diamant detects the routed host IP by
default; use `--host-ip` or `SUNTITLE_HOST_IP` to override it.

The proxy resolves the host name to a Kubernetes host resource and returns its
UID. Diamant reports health every 30 seconds and reconnects when the connection
ends. The health interval can be changed with `--health-interval-seconds` or
`SUNTITLE_HEALTH_INTERVAL_SECONDS`.

Host identity, health, desired nodes, and dispatch status persist in Kubernetes resources.
Disconnected hosts become offline; missing health reports expire after 45 seconds,
so configure the health interval below that limit. Nodes remain available through
the API while the host is offline. CI publishes
Diamant as `diamant.<target>.tar.xz` alongside the proxy release.

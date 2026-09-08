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

## Local database and independent services

Docker Compose runs PostgreSQL 18; the Rust binaries run on the host:

```bash
cp .env.example .env.local  # first setup only; credentials are for local development
just db-up
just db-migrate
just api                  # terminal 1, port 3031
just proxy                # terminal 2, port 3030
```

Both binaries read `.env.local` without overriding exported environment variables.
They require `DATABASE_URL`, a reachable database, and the exact migrated schema;
startup fails otherwise. Each uses a bounded pool of eight connections. Only the
proxy listens for database notifications. Run one proxy instance per database;
a database advisory lock rejects a second instance.

`just local-stack` prepares the database and starts API, proxy, Diamant, and UI.
It waits for the API and proxy before starting clients. Ctrl+C stops application
children and leaves PostgreSQL running. `just db-stop` stops PostgreSQL while
preserving its volume. `just ui` starts the database, API, and UI.

Commands are committed with desired state, delivered in revision order per node,
and completed only by a matching terminal runtime acknowledgement. Duplicate
commands are ignored by Diamant when their revision has already been applied. Recovery from
lost terminal acknowledgements is deferred. Registration reconciles a fresh agent
with the latest full desired configuration. Timer scans recover missed notifications, and session tokens
reject late messages from replaced connections. Domain models and OpenAPI stay
unchanged, including typed `NodeSettings`. Callers Get current models, modify them,
then submit direct Updates. An optimistic version check rejects stale submissions;
retries Get fresh state before reapplying changes. SQL Create, Update, and Delete
operations stay inside the PostgreSQL adapter.

Run `just db-test` for live storage checks, `just host-e2e` and `just node-e2e`
for separate-process checks. These use isolated schemas and leave development
records intact. Run E2E examples one at a time with ports 3030 and 3031 free.

For a local backup, capture a PostgreSQL custom-format dump:

```bash
docker compose exec -T postgres pg_dump -U suntitle -d suntitle -Fc > suntitle.dump
```

To restore into a separate empty database (keeping the current database intact):

```bash
docker compose exec -T postgres createdb -U suntitle suntitle_restore
docker compose exec -T postgres pg_restore -U suntitle -d suntitle_restore < suntitle.dump
```

Set `DATABASE_URL` to the restored database before starting the applications.

## Run the web UI with the API backend

Start PostgreSQL, the control API, and UI from the repository root. Press
`Ctrl+C` to stop the API and UI; PostgreSQL stays available:

```bash
just ui
```

See [`ui/README.md`](ui/README.md) for separate service commands, custom API
targets, and Cypress testing.

## Run the complete stack locally

Start each process in a separate terminal, from the repository root. Start the
consumers before the microphone because the proxy does not buffer messages for
clients that are not connected.

1. Prepare PostgreSQL with `just db-up` and `just db-migrate`, then start the
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

The proxy and control API share durable PostgreSQL host, node, and command state. The following
quickstart runs both services plus one Diamant host and installs an audio-source
node on it. Run each service command from the repository root in a separate
terminal.

1. Run `just db-up` and `just db-migrate`, then start `just proxy` and `just api`
   in separate terminals.

2. Start Diamant. Its ID file keeps the host UUID stable between restarts, and
   `--project-dir` tells it where to launch node programs:

   ```bash
   just diamant-local
   ```

3. List hosts and copy the `id` of `local-diamant`:

   ```bash
   just hosts
   ```

4. Install and start an audio-source node on that host:

   ```bash
   just install-audio-source <host-id>
   ```

   Diamant launches the audio listener immediately. Set `deviceName` to a device
   name when the system default is not the intended input.

5. List the nodes, then use the created node's `id` to inspect it:

   ```bash
   just nodes
   just node <node-id>
   ```

Use `PUT /api/v1/nodes/{nodeId}` with the complete editable node document to
change its name, enabled state, settings, or input. Diamant now runs uv through
its Rust library inside the host process. Stop, replacement, and restart of a
live Python command are deferred; such requests report a lifecycle error and
do not launch duplicates. A completed command can be restarted. Disconnecting
Diamant marks the host offline and retains desired nodes in the database;
Python child cleanup on shutdown is not yet implemented.

See [embedded uv integration](ws-proxy/EMBEDDED_UV.md) for the Rust API, CLI usage,
build requirements, and current limitations.

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
example. It prepares PostgreSQL, builds and starts `ws-proxy` and `control_api`,
starts a temporary `diamant` agent,
and checks that the host is returned by the control API:

```bash
just host-e2e
```

The example stops its application children and removes its temporary identity
file and isolated database schema when it finishes. PostgreSQL remains running.

To verify node lifecycle handling end to end, run:

```bash
just node-e2e
```

This starts the proxy/API and Diamant locally, registers audio-source and display
nodes, verifies listing and concurrent launches, and checks service restart
replay. Real embedded uv runs a dependency-free Python fixture, so the test
does not use audio hardware, models, or a graphical session. Fixtures exit
themselves before Diamant disconnects; this does not test deferred child
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

On its first run, Diamant creates `diamant.id` containing the host UUID and
reuses it on subsequent registrations. Use `--id-file` or `SUNTITLE_ID_FILE` to
choose another location. It reports health every 30 seconds, keeps the
WebSocket open until the connection ends, and then exits. The health interval
can be changed with `--health-interval-seconds` or
`SUNTITLE_HEALTH_INTERVAL_SECONDS`.

Host identity, health, desired nodes, and pending commands persist in PostgreSQL.
Disconnected hosts become offline; missing health reports expire after 45 seconds,
so configure the health interval below that limit. Nodes remain available through
the API while the host is offline. CI publishes
Diamant as `diamant.<target>.tar.xz` alongside the proxy release.

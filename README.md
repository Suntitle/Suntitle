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

- Python 3.10 and [uv](https://docs.astral.sh/uv/)
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

## Run the web UI with the API backend

Start the WebSocket proxy/control API and UI from the repository root. Press
`Ctrl+C` to stop both processes:

```bash
just ui
```

See [`ui/README.md`](ui/README.md) for separate service commands, custom API
targets, and Cypress testing.

## Run the complete stack locally

Start each process in a separate terminal, from the repository root. Start the
consumers before the microphone because the proxy does not buffer messages for
clients that are not connected.

1. Start the WebSocket proxy on port `3030`. This also starts the control API
   on port `3031`, sharing the proxy's in-memory host registry:

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

The proxy and control API share in-memory host and node state. The following
quickstart runs both services plus one Diamant host and installs an audio-source
node on it. Run each service command from the repository root in a separate
terminal.

1. Start the WebSocket proxy and control API:

   ```bash
   just proxy
   ```

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

5. List the nodes, then use the created node's `id` to inspect or restart it:

   ```bash
   just nodes
   just node <node-id>
   just restart-node <node-id>
   ```

Use `PUT /api/v1/nodes/{nodeId}` with the complete editable node document to
change its name, enabled state, settings, or input. Updating a running node
replaces its process. Stopping Diamant stops its node processes, marks the host
offline, and removes all nodes assigned to that host from the process-local API.

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
example. It builds and starts `ws-proxy`, starts a temporary `diamant` agent,
and checks that the host is returned by the control API:

```bash
just host-e2e
```

The example stops both child processes and removes its temporary identity file
when it finishes.

To verify node lifecycle handling end to end, run:

```bash
just node-e2e
```

This starts the proxy/API and Diamant locally, registers audio-source and display
nodes, verifies listing, restart and update behavior, adds another node, and
then confirms that stopping Diamant removes all of its nodes. Node processes are
replaced by a lightweight fixture, so the test does not use audio hardware,
models, or a graphical session. The lifecycle test currently requires Unix
process signals.

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

The proxy currently returns a complete online host record but does not persist
it in the control API; that storage remains an explicit follow-up. CI publishes
Diamant as `diamant.<target>.tar.xz` alongside the proxy release.

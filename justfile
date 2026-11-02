set shell := ["bash", "-uc"]

# Show the available project commands.
default:
    @just --list

# Install the Python dependencies.
setup:
    uv sync

# Start the WebSocket proxy in release mode.
proxy-release:
    cargo run --release --manifest-path ws-proxy/Cargo.toml --bin ws-proxy -- --kubeconfig pki/suntitle-proxy.kubeconfig

# Start the WebSocket proxy in development mode.
proxy:
    cargo run --manifest-path ws-proxy/Cargo.toml --bin ws-proxy -- --kubeconfig pki/suntitle-proxy.kubeconfig

# Create local database configuration without overwriting existing settings.
db-env:
    @if [[ ! -e .env.local ]]; then cp .env.example .env.local; fi

# Start PostgreSQL and wait until it is healthy.
db-up: db-env
    docker compose --env-file .env.local up -d --wait --wait-timeout 90 postgres

# Stop PostgreSQL, retaining its data volume.
db-stop:
    docker compose stop postgres

# Stop Compose services and permanently delete the local database and Kubernetes data.
db-drop-volume: db-env
    docker compose --env-file .env.local down --volumes

# Start the independent control API.
api:
    cargo run --manifest-path ws-proxy/Cargo.toml --bin control_api -- --kubeconfig pki/suntitle-api.kubeconfig

# Start the subtitle window.
subtitle host="localhost:3030":
    uv run python suntitle.py --host "{{ host }}"

# Start the Ukrainian-to-Czech translator.
translator host="localhost:3030":
    uv run python suntitle-translator.py --host "{{ host }}"

# Start the Whisper transcriber.
transcriber host="localhost:3030" model="small" language="auto":
    uv run python whisper_online_client.py --uri "ws://{{ host }}/transcribe" --model "{{ model }}" --language "{{ language }}"

# Show the Whisper transcriber options.
transcriber-help:
    uv run python whisper_online_client.py --help

# Stream the default microphone to the proxy.
listener host="localhost:3030":
    uv run python suntitle-listener.py "ws://{{ host }}"

# List available microphone input devices.
listener-devices:
    uv run python suntitle-listener.py --list-devices

# Stream a selected microphone to the proxy.
listener-device device host="localhost:3030":
    uv run python suntitle-listener.py "ws://{{ host }}" --device "{{ device }}"

# Start the local Diamant host as local-host.
diamant-local:
    cargo run --manifest-path ws-proxy/Cargo.toml --bin diamant -- --cli --host-name local-host --project-dir "{{ justfile_directory() }}"

# Run kubectl against the local Compose API server, forwarding all arguments.
[positional-arguments]
kubectl *args:
    @kubectl --kubeconfig=/dev/null --server=https://127.0.0.1:6443 --certificate-authority=pki/ca.crt --client-certificate=pki/admin.crt --client-key=pki/admin.key "$@"

# Start the Compose services after checking their required certificate files.
_local-infra-up: db-env
    #!/usr/bin/env bash
    set -euo pipefail
    for file in ca.crt apiserver.crt apiserver.key sa.pub sa.key admin.crt admin.key; do
        if [[ ! -s "pki/$file" ]]; then
            echo "Missing pki/$file. Run 'just generate-certs' before 'just local-stack'." >&2
            exit 1
        fi
    done
    bash scripts/generate-kubeconfigs.sh
    docker compose --env-file .env.local build kubernetes-bootstrap ui
    docker compose --env-file .env.local up -d --wait --wait-timeout 180

# Start Kubernetes and install resource contracts for local applications and E2Es.
kubernetes-up: db-env
    bash scripts/generate-kubeconfigs.sh
    docker compose --env-file .env.local up -d kube-apiserver
    @for attempt in {1..90}; do just kubectl get --raw=/readyz >/dev/null 2>&1 && break; sleep 1; done
    just kubernetes-bootstrap

# Start the full stack. Exit stops all services and preserves database data.
local-stack:
    #!/usr/bin/env bash
    set -euo pipefail

    pids=()

    cleanup() {
        status=$?
        trap - EXIT
        trap '' INT TERM
        set +e
        if (( ${#pids[@]} > 0 )); then
            kill -TERM "${pids[@]}" 2>/dev/null
            wait "${pids[@]}" 2>/dev/null
        fi
        docker compose --env-file .env.local down
        exit "$status"
    }
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    if [[ "$(uname -s)" == Darwin && -z "${SDKROOT:-}" ]]; then
        export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"
    fi

    just _local-infra-up
    cargo build --manifest-path ws-proxy/Cargo.toml --bin diamant
    ws-proxy/target/debug/diamant --cli --proxy-url ws://localhost/register --host-name local-host --project-dir "{{ justfile_directory() }}" &
    pids+=("$!")

    while true; do
        for pid in "${pids[@]}"; do
            if ! kill -0 "$pid" 2>/dev/null; then
                wait "$pid"
                exit 1
            fi
        done
        sleep 1
    done

# Start a release-mode Diamant host agent.
diamant proxy_url="ws://localhost/register":
    cargo run --release --manifest-path ws-proxy/Cargo.toml --bin diamant -- --cli --proxy-url "{{ proxy_url }}"

# List registered hosts from the control API.
hosts:
    curl -sS http://127.0.0.1:3031/api/v1/hosts | python3 -m json.tool

# Install and start a default audio-source node on a host.
install-audio-source host_id host_name:
    curl -sS -X POST "http://127.0.0.1:3031/api/v1/hosts/{{ host_id }}/nodes" -H 'Content-Type: application/json' -d '{"spec":{"hostRef":{"name":"{{ host_name }}"},"inputRef":null,"restartToken":null,"name":"Local microphone","description":"Default local audio input","enabled":true,"settings":{"audio_source":{"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le","deviceName":null}}}}' | python3 -m json.tool

# List all nodes from the control API.
nodes:
    curl -sS http://127.0.0.1:3031/api/v1/nodes | python3 -m json.tool

# Get one node from the control API.
node node_id:
    curl -sS "http://127.0.0.1:3031/api/v1/nodes/{{ node_id }}" | python3 -m json.tool

# Restart one node through the control API.
restart-node node_id:
    curl -sS -X POST "http://127.0.0.1:3031/api/v1/nodes/{{ node_id }}/restart" | python3 -m json.tool

# Run the lightweight WebSocket relay end-to-end probe.
e2e:
    cargo run --locked --manifest-path ws-proxy/Cargo.toml --example e2e_probe

# Run the host-registration end-to-end test.
host-e2e: kubernetes-up
    cargo run --locked --manifest-path ws-proxy/Cargo.toml --example host_registration_e2e

# Run the node-lifecycle end-to-end test.
node-e2e: kubernetes-up
    cargo run --locked --manifest-path ws-proxy/Cargo.toml --example node_handling_e2e

# Build and retain the native development extension for the current host architecture.
custom-node-fixtures:
    cargo build --locked --manifest-path ws-proxy/Cargo.toml --target-dir ws-proxy/target --example custom_node_probe
    cp ws-proxy/target/debug/examples/custom_node_probe ws-proxy/examples/custom_node_e2e/extensions/native/bin/probe

# Verify local and OCI native, Python script, and Python module extensions through the API.
custom-node-e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    bash scripts/generate-kubeconfigs.sh
    compose=(docker compose -p suntitle-custom-e2e -f compose.yaml -f compose.e2e.yaml)
    cleanup() {
        status=$?
        trap - EXIT
        if (( status != 0 )); then "${compose[@]}" logs --no-color; fi
        "${compose[@]}" down --volumes
        exit "$status"
    }
    trap cleanup EXIT
    "${compose[@]}" up -d --wait --wait-timeout 90 kube-apiserver distribution traefik
    cargo run --locked --manifest-path ws-proxy/Cargo.toml --example custom_node_e2e

# Run Rust tests that do not require a Kubernetes server.
test-rust:
    cargo test --locked --manifest-path ws-proxy/Cargo.toml

# Run ignored integration tests against bootstrapped Kubernetes.
test-rust-live:
    cargo test --locked --manifest-path ws-proxy/Cargo.toml -- --ignored --test-threads=1

# Run the mock API and node CLI tests.
test-python python="python3":
    {{quote(python)}} -m unittest discover -s tests -v

# Select an existing Pulumi stack.
infra-select stack="dev":
    pulumi -C infrastructure stack select "{{ stack }}"

# Initialize a new Pulumi stack.
infra-init stack="dev":
    pulumi -C infrastructure stack init "{{ stack }}"

# Preview the Pulumi deployment.
infra-preview:
    pulumi -C infrastructure preview

# Deploy the Pulumi stack.
infra-up:
    pulumi -C infrastructure up

# Set the Pulumi stack's AWS region.
infra-region region:
    pulumi -C infrastructure config set aws:region "{{ region }}"

# Print the deployed proxy's public IP.
infra-ip:
    pulumi -C infrastructure stack output publicIp

# Destroy the Pulumi stack's AWS resources.
infra-destroy:
    pulumi -C infrastructure destroy

# Install the UI dependencies when they are not already present.
_ui-dependencies:
    @if [[ ! -x ui/node_modules/.bin/vite ]]; then npm --prefix ui install; fi

# Start the UI and control API together. Ctrl+C stops both processes.
ui: _ui-dependencies kubernetes-up
    #!/usr/bin/env bash
    set -euo pipefail

    cargo build --manifest-path ws-proxy/Cargo.toml --bin control_api
    ws-proxy/target/debug/control_api --kubeconfig pki/suntitle-api.kubeconfig &
    backend_pid=$!

    cleanup() {
        kill "$backend_pid" 2>/dev/null || true
        wait "$backend_pid" 2>/dev/null || true
    }
    trap cleanup EXIT INT TERM

    (cd ui && exec env SUNTITLE_API_PROXY_TARGET=http://127.0.0.1:3031 node node_modules/vite/bin/vite.js)

# Start the UI and mock API together. Ctrl+C stops both processes.
ui-mock: _ui-dependencies
    #!/usr/bin/env bash
    set -euo pipefail

    python3 mockserver/server.py &
    api_pid=$!

    cleanup() {
        kill "$api_pid" 2>/dev/null || true
        wait "$api_pid" 2>/dev/null || true
    }
    trap cleanup EXIT INT TERM

    npm --prefix ui run dev

# Start only the mock API.
mock-api:
    python3 mockserver/server.py

# Start only the UI development server.
ui-dev: _ui-dependencies
    npm --prefix ui run dev

# Start only the UI development server, proxied to the control API.
ui-backend: _ui-dependencies
    SUNTITLE_API_PROXY_TARGET=http://127.0.0.1:3031 npm --prefix ui run dev

# Create the UI's local environment file without overwriting an existing one.
ui-env:
    @if [[ -e ui/.env.local ]]; then echo "ui/.env.local already exists"; else cp ui/.env.example ui/.env.local; fi

# Run the headless Cypress integration suite.
ui-test: _ui-dependencies
    npm --prefix ui run test:e2e

# Start the test server and run selected Cypress specs (paths relative to ui/).
ui-test-spec spec: _ui-dependencies
    CYPRESS_specPattern={{quote(spec)}} npm --prefix ui run test:e2e

# Run Cypress against an existing test server, optionally selecting specs.
ui-test-run spec="cypress/e2e/**/*.cy.ts": _ui-dependencies
    npm --prefix ui run cypress:run -- --spec {{quote(spec)}}

# Start the UI server used for interactive Cypress debugging.
ui-test-server: _ui-dependencies
    npm --prefix ui run dev:test

# Open Cypress for interactive debugging.
ui-test-open: _ui-dependencies
    npm --prefix ui run cypress:open

generate-certs:
    #!/usr/bin/env bash
    ./scripts/generate-certs.sh

# Generate the phase 1 Kubernetes custom-resource definitions without a server.
generate-crds:
    cargo run --manifest-path ws-proxy/Cargo.toml --bin control_api -- generate-crds > kubernetes/crds.yaml

# Create separate local certificate kubeconfigs, retaining existing client keys.
generate-kubeconfigs:
    bash scripts/generate-kubeconfigs.sh

# Install namespace, CRDs and runtime RBAC using explicit bootstrap credentials.
kubernetes-bootstrap kubeconfig="pki/admin.kubeconfig" namespace="suntitle":
    cargo run --manifest-path ws-proxy/Cargo.toml --bin control_api -- bootstrap-kubernetes --kubeconfig {{quote(kubeconfig)}} --namespace {{quote(namespace)}}

# Exercise new resource contracts on the running standalone Kubernetes API.
kubernetes-test:
    python3 scripts/test-kubernetes-resources.py

fmt:
    cargo fmt --manifest-path ws-proxy/Cargo.toml

clippy:
    cargo clippy --manifest-path ws-proxy/Cargo.toml --fix --allow-dirty

set shell := ["bash", "-uc"]

# Show the available project commands.
default:
    @just --list

# Install the Python dependencies.
setup:
    uv sync

# Start the WebSocket proxy in release mode.
proxy-release:
    cargo run --release --manifest-path ws-proxy/Cargo.toml --bin ws-proxy

# Start the WebSocket proxy in development mode.
proxy:
    cargo run --manifest-path ws-proxy/Cargo.toml --bin ws-proxy

# Create local database configuration without overwriting existing settings.
db-env:
    @if [[ ! -e .env.local ]]; then cp .env.example .env.local; fi

# Start PostgreSQL and wait until it is healthy.
db-up: db-env
    docker compose up -d --wait --wait-timeout 90 postgres

# Apply versioned database migrations.
db-migrate:
    cargo run --manifest-path ws-proxy/Cargo.toml --bin db_migrate

# Run live storage tests in isolated schemas.
db-test: db-up db-migrate
    cargo test --manifest-path ws-proxy/Cargo.toml --test postgres_store -- --ignored

# Stop PostgreSQL, retaining its data volume.
db-stop:
    docker compose stop postgres

# Start the independent control API.
api:
    cargo run --manifest-path ws-proxy/Cargo.toml --bin control_api

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

# Start a local Diamant host with a persistent project-local identity.
diamant-local:
    cargo run --manifest-path ws-proxy/Cargo.toml --bin diamant -- --id-file .suntitle/diamant.id --project-dir "{{ justfile_directory() }}"

# Run kubectl against the local Compose API server, forwarding all arguments.
[positional-arguments]
kubectl *args:
    @kubectl --kubeconfig=/dev/null --server=https://127.0.0.1:6443 --certificate-authority=pki/ca.crt --client-certificate=pki/admin.crt --client-key=pki/admin.key "$@"

# Start the Compose services after checking their required certificate files.
_local-infra-up: db-env
    #!/usr/bin/env bash
    set -euo pipefail
    for file in ca.crt apiserver.crt apiserver.key sa.pub sa.key; do
        if [[ ! -s "pki/$file" ]]; then
            echo "Missing pki/$file. Run 'just generate-certs' before 'just local-stack'." >&2
            exit 1
        fi
    done
    docker compose --env-file .env.local up -d --wait --wait-timeout 90 postgres kine kube-apiserver

# Start PostgreSQL, Kine, kube-apiserver, API, proxy, Diamant and UI. Ctrl+C leaves Compose services running.
local-stack: _local-infra-up _ui-dependencies db-migrate
    #!/usr/bin/env bash
    set -euo pipefail

    pids=()

    cleanup() {
        trap - EXIT
        if (( ${#pids[@]} > 0 )); then
            kill "${pids[@]}" 2>/dev/null || true
            wait "${pids[@]}" 2>/dev/null || true
        fi
    }
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    cargo build --manifest-path ws-proxy/Cargo.toml --bins
    ws-proxy/target/debug/control_api &
    api_pid=$!
    pids+=("$api_pid")

    ws-proxy/target/debug/ws-proxy &
    proxy_pid=$!
    pids+=("$proxy_pid")

    for _ in {1..120}; do
        if curl -fsS http://127.0.0.1:3031/api/v1/hosts >/dev/null 2>&1 && (echo > /dev/tcp/127.0.0.1/3030) 2>/dev/null; then
            break
        fi
        if ! kill -0 "$api_pid" 2>/dev/null; then
            wait "$api_pid"
            exit 1
        fi
        if ! kill -0 "$proxy_pid" 2>/dev/null; then
            wait "$proxy_pid"
            exit 1
        fi
        sleep 0.25
    done
    curl -fsS http://127.0.0.1:3031/api/v1/hosts >/dev/null
    (echo > /dev/tcp/127.0.0.1/3030) 2>/dev/null

    ws-proxy/target/debug/diamant --id-file .suntitle/diamant.id --project-dir "{{ justfile_directory() }}" &
    pids+=("$!")

    (cd ui && exec env SUNTITLE_API_PROXY_TARGET=http://127.0.0.1:3031 node node_modules/vite/bin/vite.js) &
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
diamant proxy_url="ws://localhost:3030/register":
    cargo run --release --manifest-path ws-proxy/Cargo.toml --bin diamant -- --proxy-url "{{ proxy_url }}"

# List registered hosts from the control API.
hosts:
    curl -sS http://127.0.0.1:3031/api/v1/hosts | python3 -m json.tool

# Install and start a default audio-source node on a host.
install-audio-source host_id:
    curl -sS -X POST "http://127.0.0.1:3031/api/v1/hosts/{{ host_id }}/nodes" -H 'Content-Type: application/json' -d '{"type":"audio_source","inputNodeId":null,"name":"Local microphone","description":"Default local audio input","enabled":true,"settings":{"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le","deviceName":null}}' | python3 -m json.tool

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
    cargo run --manifest-path ws-proxy/Cargo.toml --example e2e_probe

# Run the host-registration end-to-end test.
host-e2e: db-up db-migrate
    cargo run --manifest-path ws-proxy/Cargo.toml --example host_registration_e2e

# Run the node-lifecycle end-to-end test.
node-e2e: db-up db-migrate
    cargo run --manifest-path ws-proxy/Cargo.toml --example node_handling_e2e

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
ui: _ui-dependencies db-up db-migrate
    #!/usr/bin/env bash
    set -euo pipefail

    cargo build --manifest-path ws-proxy/Cargo.toml --bin control_api
    ws-proxy/target/debug/control_api &
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
set shell := ["bash", "-uc"]

# Show the available project commands.
default:
    @just --list

# Install the Python dependencies.
setup:
    uv sync

# Start the WebSocket proxy and control API in release mode.
proxy-release:
    cargo run --release --manifest-path ws-proxy/Cargo.toml --bin ws-proxy

# Start the WebSocket proxy and control API in development mode.
proxy:
    cargo run --manifest-path ws-proxy/Cargo.toml --bin ws-proxy

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

# Start the UI, WebSocket proxy/control API, and Diamant locally. Ctrl+C stops all processes.
local-stack: _ui-dependencies
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

    cargo run --manifest-path ws-proxy/Cargo.toml --bin ws-proxy &
    proxy_pid=$!
    pids+=("$proxy_pid")

    for _ in {1..120}; do
        if curl -fsS http://127.0.0.1:3031/api/v1/hosts >/dev/null 2>&1; then
            break
        fi
        if ! kill -0 "$proxy_pid" 2>/dev/null; then
            wait "$proxy_pid"
        fi
        sleep 0.25
    done
    curl -fsS http://127.0.0.1:3031/api/v1/hosts >/dev/null

    cargo run --manifest-path ws-proxy/Cargo.toml --bin diamant -- --id-file .suntitle/diamant.id --project-dir "{{ justfile_directory() }}" &
    pids+=("$!")

    SUNTITLE_API_PROXY_TARGET=http://127.0.0.1:3031 npm --prefix ui run dev &
    pids+=("$!")

    while true; do
        for pid in "${pids[@]}"; do
            if ! kill -0 "$pid" 2>/dev/null; then
                wait "$pid"
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
    curl -sS -X POST "http://127.0.0.1:3031/api/v1/hosts/{{ host_id }}/nodes" -H 'Content-Type: application/json' -d '{"type":"audio_source","name":"Local microphone","description":"Default local audio input","enabled":true,"settings":{"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le","deviceName":null}}' | python3 -m json.tool

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
host-e2e:
    cargo run --manifest-path ws-proxy/Cargo.toml --example host_registration_e2e

# Run the node-lifecycle end-to-end test.
node-e2e:
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
ui: _ui-dependencies
    #!/usr/bin/env bash
    set -euo pipefail

    cargo run --manifest-path ws-proxy/Cargo.toml --bin ws-proxy &
    backend_pid=$!

    cleanup() {
        kill "$backend_pid" 2>/dev/null || true
        wait "$backend_pid" 2>/dev/null || true
    }
    trap cleanup EXIT INT TERM

    SUNTITLE_API_PROXY_TARGET=http://127.0.0.1:3031 npm --prefix ui run dev

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

# Start the UI server used for interactive Cypress debugging.
ui-test-server: _ui-dependencies
    npm --prefix ui run dev:test

# Open Cypress for interactive debugging.
ui-test-open: _ui-dependencies
    npm --prefix ui run cypress:open

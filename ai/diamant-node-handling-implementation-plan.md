# Diamant node handling implementation plan

## Goal

Extend `diamant` from a host-presence agent into the owner of the nodes assigned
to its host. While Diamant is running, it must:

- receive node assignments over its existing `/register` WebSocket;
- keep each node's ID, latest `NodeUpdate`, and child process in memory;
- translate the node type and settings into the corresponding `uv run python ...`
  command;
- start, stop, replace, and restart a child process idempotently;
- report lifecycle results to the proxy; and
- gracefully stop every child process when Diamant shuts down, with a forced-kill
  fallback after a bounded grace period.

This is runtime orchestration work only. Node persistence across Diamant restarts
is not included; the proxy remains the source of truth and must replay the desired
node set after registration or reconnection.

## Important constraints discovered in the current code

- `diamant` currently registers the host and sends health reports, but discards
  all subsequent server messages.
- `RegistrationServerMessage` currently contains only `Registered` and `Error`;
  the protocol has no node assignment or lifecycle acknowledgement messages.
- The REST node handlers and a node registry are not implemented yet. The proxy
  only has an in-memory `HostRegistry`.
- `NodeUpdate` has no node ID, so the registration command must carry the assigned
  UUID separately.
- The Python entry points must run from the repository/project directory so `uv`
  can find `pyproject.toml` and the scripts. Diamant needs an explicit configurable
  project directory rather than relying on its launch directory.
- The current API settings cannot all be honored by the current scripts:
  - `suntitle-listener.py` sends PCM16 only; `pcm_f32le` and `opus` are not
    implemented.
  - `whisper_online_client.py` accepts model, language, task, and `--vad`, but the
    beam size is hard-coded to `5` in `whisper_online.py`.
  - `suntitle-translator.py` is fixed to `Helsinki-NLP/opus-mt-uk-cs`; it does not
    accept source language, target language, provider, model, or formatting flags.
  - `suntitle.py` accepts only the proxy host and does not accept display settings.
- The audio listener's documented working endpoint is the proxy root (`/`), while
  the script's default/help mention `/audio`, which the proxy does not expose.
- A `Drop` implementation alone cannot provide fully asynchronous graceful
  shutdown. The manager needs an explicit shutdown path that can wait for children,
  plus `Drop` as a last-resort safety net.

## Proposed architecture

### 1. Extend the registration protocol

Keep host presence and node control on the existing full-duplex registration
WebSocket. Add commands sent by the proxy after `Registered`:

```rust
RegistrationServerMessage::ApplyNode {
    node_id: Uuid,
    revision: u64,
    update: NodeUpdate,
}
RegistrationServerMessage::RemoveNode {
    node_id: Uuid,
    revision: u64,
}
RegistrationServerMessage::RestartNode {
    node_id: Uuid,
    revision: u64,
}
```

Add lifecycle messages from Diamant to the proxy:

```rust
RegistrationClientMessage::NodeStatus {
    host_id: Uuid,
    node_id: Uuid,
    revision: u64,
    status: NodeRuntimeStatus,
    message: Option<String>,
}
```

`NodeRuntimeStatus` should distinguish at least `Starting`, `Running`, `Stopped`,
and `Failed`. The monotonically increasing revision lets Diamant ignore stale or
duplicated commands and lets the proxy correlate acknowledgements with the desired
configuration.

After acknowledging host registration, the proxy must send an `ApplyNode` snapshot
for every node currently assigned to that host. Live create/update/enable/disable
and restart operations then use the same messages. This makes reconnect recovery
deterministic without persisting runtime state in Diamant.

### 2. Introduce an in-memory node process manager

Move orchestration out of `bin/diamant.rs` into a testable module, for example
`ws-proxy/src/diamant/nodes.rs`:

```rust
struct NodeProcessManager {
    project_dir: PathBuf,
    data_plane_url: String,
    shutdown_grace: Duration,
    nodes: HashMap<Uuid, ManagedNode>,
}

struct ManagedNode {
    revision: u64,
    update: NodeUpdate,
    child: Option<ChildProcess>,
}
```

The map entry is the in-memory node registration. Keeping `child` optional allows
disabled nodes to remain registered without running a process.

Expose narrow operations:

- `apply(node_id, revision, update)` inserts a new node or reconciles an existing
  one;
- `remove(node_id, revision)` gracefully stops and removes it;
- `restart(node_id, revision)` restarts it using the stored update;
- `reap_exited()` detects unexpected exits and returns failure events;
- `shutdown_all().await` stops and waits for every process.

Reconciliation rules:

1. Ignore commands older than the stored revision.
2. An identical update at the same revision is a no-op.
3. `enabled: false` registers the node but leaves `child: None`.
4. A new enabled node starts one process.
5. Changing any process-affecting field gracefully replaces the old process.
6. A spawn failure retains the desired update with no child and reports `Failed`.
7. An unexpected child exit is reaped and reported; do not create an unbounded
   automatic restart loop in the first implementation.

### 3. Build commands without invoking a shell

Use `tokio::process::Command` (or a small wrapper around it) with separate arguments,
never a shell command string. Set:

- executable: `uv` (configurable with `--uv-bin` / `UV_BIN` for tests and
  nonstandard installations);
- current directory: `--project-dir` / `PROJECT_DIR`;
- stdin: null;
- stdout/stderr: inherited initially so node failures remain observable;
- kill-on-drop: enabled only as a final fallback, not as the normal shutdown path.

Add `--data-plane-url` / `DATA_PLANE_URL`, defaulting to the origin of the
registration URL with `/register` removed. Validate the result once at startup.
Keeping it separately configurable avoids deriving incorrect endpoints when the
registration path is routed through a gateway.

The intended command mapping is:

| Node type | Command and arguments |
| --- | --- |
| Audio source | `uv run python suntitle-listener.py <base>/ --samplerate <sampleRateHz> --channels <channels> [--device <deviceName>] --format raw` |
| Transcriber | `uv run python whisper_online_client.py --uri <base>/transcribe --model <model> --language <language> --task <task> [--vad] --beam-size <beamSize>` |
| Translator | `uv run python suntitle-translator.py --host <authority> --source-language <sourceLanguage> --target-language <targetLanguage> --provider <provider> [--model <model>] [--no-preserve-formatting]` |
| Display | `uv run python suntitle.py --host <authority> --max-lines <maxLines> --font-scale <fontScale> [--show-original] --theme <theme>` |

The URL conversion helpers must preserve `ws` versus `wss`, host, optional port,
and any configured path prefix. Use typed URL parsing rather than string replacement.

Before enabling all four mappings, add the missing Python CLI options and wire them
to actual behavior. Until a setting is supported, command construction must return
a clear validation error instead of silently ignoring it. In particular:

- initially accept only `pcm_s16le`, unless the listener gains real f32/Opus output;
- expose `beam_size` through `whisper_online.py` and
  `whisper_online_client.py`;
- make translator model/provider/language selection configurable, defining the
  supported provider/model combinations explicitly; and
- make display settings affect the PyQt window, including a clear definition of
  how `showOriginal` is represented by the incoming message format.

Normalize BCP 47 tags where a Python backend requires a shorter language code, or
reject unsupported tags with a precise error. Do not silently truncate a tag.

### 4. Implement graceful child ownership

Wrap each child in a `ChildProcess` type responsible for termination and reaping.
On Unix, launch each `uv` command in its own process group so shutdown reaches both
the `uv` wrapper and Python descendants.

Normal shutdown sequence for one child:

1. If already exited, reap it and return its status.
2. Send `SIGTERM` to the process group.
3. Wait up to the configured grace period.
4. If it is still alive, send `SIGKILL` to the process group.
5. Wait/reap it so no zombie remains.

`NodeProcessManager::shutdown_all()` should initiate termination for every child
before waiting, so one slow child does not consume the grace period of every other
child serially.

Implement `Drop` for the owning manager or child wrapper only as a synchronous,
best-effort backstop: terminate any still-owned process groups and reap what can be
reaped. The ordinary application path must call and await `shutdown_all()` before
the manager is dropped. This split gives deterministic graceful shutdown while
still protecting early-return and panic paths.

Document platform behavior. If Windows support is required, add a platform-specific
job-object implementation; otherwise gate the process-group code to Unix and state
that the first implementation supports Unix hosts.

### 5. Refactor the Diamant event loop

Replace the current pair of forwarded futures with one event loop that owns the
WebSocket writer and `NodeProcessManager`. Use `tokio::select!` over:

- the health-report interval;
- the next registration/control WebSocket message;
- a child-reaping interval or child exit notifications; and
- `tokio::signal::ctrl_c()` plus Unix `SIGTERM`.

On a shutdown signal, WebSocket close, protocol error, or fatal send/receive error:

1. stop accepting node commands;
2. call `shutdown_all().await`;
3. best-effort send final `Stopped` statuses while the socket is usable;
4. send a WebSocket close frame when possible; and
5. return the original error after cleanup, if shutdown was caused by an error.

The manager must remain in the outer scope so every return path reaches cleanup.
Do not hold a map lock across process spawn, termination, or `.await`; a single-owner
event loop can avoid a lock entirely.

## Proxy/control API integration

Diamant cannot receive assignments until the proxy owns node desired state and a
sender for each registered host. Add shared registries alongside `HostRegistry`:

- `NodeRegistry`: desired/current node records keyed by node UUID;
- `HostControlRegistry`: outbound registration-channel sender keyed by host UUID.

Then implement the minimum control path:

1. `POST /hosts/{hostId}/nodes` creates the UUID and desired node state, stores it,
   and sends `ApplyNode` to the connected host.
2. `PUT /nodes/{nodeId}` validates immutable type/host, increments the revision,
   stores the desired update, and sends `ApplyNode`. Pipeline input references do
   not gate startup or restart the runnable; pipeline routing is separate work.
3. Enabling/disabling uses the same `ApplyNode` reconciliation.
4. `POST /nodes/{nodeId}/restart` sends `RestartNode` with a new revision.
5. Diamant status messages update the API-facing `NodeStatus` and error metadata.
6. Host registration installs/removes its control sender and replays its desired
   node snapshot after the `Registered` response.

If the REST node handlers are intentionally being implemented in another task,
split this work at the protocol boundary: provide a tested proxy-side function
that can send commands through `HostControlRegistry`, then let the API task call it.

## Implementation phases

### Phase 1: protocol and pure command planning

- Add the node control/status wire types and serialization tests.
- Add typed helpers for extracting a data-plane base URL and authority.
- Implement a pure `command_spec(&NodeUpdate)` function returning executable,
  arguments, and working directory.
- Unit-test every node type, optional argument, disabled node, unsupported setting,
  URL scheme, path prefix, and argument containing whitespace/shell characters.

### Phase 2: child lifecycle manager

- Implement `ChildProcess` and `NodeProcessManager`.
- Add idempotent apply/remove/restart semantics and revision handling.
- Add bounded graceful shutdown, process-group termination, reaping, and `Drop`
  fallback.
- Test with a small fixture executable/script that records arguments and termination;
  do not load ML models, PyQt, or audio devices in lifecycle tests.

### Phase 3: Diamant loop integration

- Consume node commands after registration instead of discarding messages.
- Add health, status acknowledgement, child-exit, and OS-shutdown branches.
- Ensure every exit path invokes manager cleanup.
- Add structured logs containing host ID, node ID, revision, type, PID, and exit
  status, without logging secrets or the full environment.

### Phase 4: executable compatibility

- Add and test the missing Python CLI options needed by the mappings above.
- Correct the listener endpoint documentation/default mismatch.
- Validate unsupported encodings, languages, providers, and display behavior before
  spawning.
- Add lightweight Python argument-parsing tests that do not initialize hardware,
  models, or GUI components; this may require moving module-level startup work into
  `main()` functions.

### Phase 5: proxy bridge and end-to-end verification

- Add node and host-control registries to shared application state.
- Connect create/update/restart handlers to registration commands and status events.
- Extend the existing host-registration E2E fixture with a fake `uv` executable.
- Verify create starts once, repeated apply is a no-op, update replaces, disable
  stops, restart restarts, remove deletes, unexpected exit reports failure, and
  Diamant termination gracefully stops every child.

## Test matrix

### Rust unit tests

- JSON round trips for every new protocol message.
- Exact argument vectors for all four `NodeUpdate` variants.
- No shell interpretation of names/settings containing spaces or metacharacters.
- Same/stale revision does not duplicate a process.
- Disabled apply stores configuration without spawning.
- Updated configuration terminates and replaces the old child.
- Failed spawn and unexpected exit are retained/reported correctly.
- Remove and restart of unknown nodes produce explicit, nonfatal failures.
- `shutdown_all()` terminates multiple children concurrently and reaps them.

### Integration tests

- Registration acknowledgement is followed by desired-node snapshot replay.
- A fake `uv` receives the expected working directory and exact arguments.
- REST create/update/disable/restart produces the matching Diamant lifecycle.
- WebSocket loss and `SIGTERM` both clean up child process groups.
- A child that ignores `SIGTERM` is killed after the grace timeout.
- No child or grandchild remains after Diamant exits.

### Verification commands

E2E preparation is owned by `just e2e-build`, which builds applications and examples
before either `host-e2e` or `node-e2e` starts its harness. Direct Cargo example runs
MUST prepare application binaries first. The custom-node recipe additionally
depends on `custom-node-fixtures` to retain its native payload for development.
Shared process guards, binary discovery, startup waits, API requests, and polling
live under `ws-proxy/examples/support`.

The extension publication and host installation/execution sequences are in the
[custom-node plan](custom-node-type-plan.md#registration-preparation-and-execution-sequence).

```bash
cargo fmt --manifest-path ws-proxy/Cargo.toml --check
cargo clippy --manifest-path ws-proxy/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path ws-proxy/Cargo.toml
```

Run any added Python parser tests through `uv run`. Keep the E2E suite independent
of real microphones, GUI sessions, external APIs, and model downloads.

## Acceptance criteria

- Each node assigned to a host has exactly one in-memory entry in Diamant.
- An enabled node has at most one owned child process; a disabled node has none.
- Every supported `NodeUpdate` field either changes the launched process behavior
  or is rejected explicitly before spawn.
- Duplicate and stale commands do not create duplicate processes.
- Process exits are reaped and reported to the proxy.
- Ctrl+C, `SIGTERM`, registration-socket loss, normal return, and error return all
  clean up every owned process; cooperative children receive `SIGTERM` before the
  forced-kill deadline.
- Proxy reconnection replays desired nodes and reconstructs Diamant's in-memory
  state without local persistence.
- Relevant Rust, Python argument, and end-to-end tests pass.
- No commit is created until the user reviews and explicitly approves the solution.

## Decisions to confirm before implementation

1. Whether the first release is Unix-only; process-group shutdown is the reliable
   way to terminate `uv` and its Python descendant together.
2. Whether unsupported API settings should be removed/narrowed in the contract or
   implemented in the Python programs. This plan recommends implementing them where
   behavior is well-defined and rejecting the rest explicitly.
3. Whether loss of the proxy registration connection should stop all nodes. This
   plan recommends yes, because the proxy is the desired-state authority and stale
   processes should not continue unmanaged.
4. Whether Diamant should reconnect automatically. The first safe increment can
   clean up and exit on disconnect; reconnect/backoff can follow without changing
   the node-manager design.

# Custom node execution and plugin contracts

## Outcome

Allow Diamant to run a custom component installed on its host. Custom components
MUST run directly unless their execution configuration explicitly selects Python.
They MUST accept arbitrary plugin settings through a declarative argument adapter
and implement the communication contract of their selected pipeline type.

This document is a proposal. It does not introduce API contracts or implementation
changes. The configuration examples describe the intended model, not an existing
API payload.

## Extension specification and node instances

An extension describes the installation source for custom plugins and their
prerequisites. The adapter YAML is the extension specification. It defines the
plugin's type, execution, settings, and CLI mappings without a separate plugin
manifest or plugin-definition resource.

| Concept | Responsibility |
| --- | --- |
| Extension | Identifies the installation source and contains the adapter specification for type, entry point, runtime, settings, and CLI mappings |
| Node | References an extension when custom, supplies instance setting values, and runs on an assigned host under Diamant supervision |

The Node CR MUST gain an extension reference for custom nodes. Built-in nodes
do not require an extension reference.

The lifecycle is:

1. Register an extension and its installation source.
2. Read and validate the extension's adapter specification.
3. Install the selected plugin and prepare its prerequisites on the assigned host.
4. Create or configure a node referencing that extension.
5. Launch the node through Diamant after host preparation succeeds.

Extension specifications SHOULD be reusable across nodes. Execution details and
CLI mappings belong to the extension; node-specific values belong to the instance.
The installation mechanism and extension update behavior still require concrete
contracts. Success and failure MUST be recorded in the corresponding CR status:
extension preparation results on the Extension CR and node execution results on
the Node CR.

The existing Extensions frontend lists package metadata and offers installation
and refresh requests. Its backend handlers are placeholders, and local file
selection currently submits a filename or path rather than uploading contents.
Implementing host installation, dependency preparation, and plugin discovery is
required to complete this lifecycle. The Extensions UI manages package sources
and adapter specifications; node configuration references extensions and supplies
instance values.

## Node, host, and extension dependencies

Extension MUST be introduced as a new Kubernetes custom resource backing the
current stub Extensions API. Add controller logic to reconcile extension
resources and coordinate source resolution and host preparation through HUB.
HUB is the API and ws-proxy, including their controller logic. Custom nodes
reference extensions through HUB. HUB resolves node and extension resources
for hosts rather than requiring hosts to access those resources directly.

```mermaid
flowchart TB
    SCRIPT((SCRIPT))
    HUB["HUB"]

    NODE["NODE"]
    HOST["HOST"]
    EXT["EXT"]

    SRC_OCI["SRC = OCI"]
    SRC_GIT["SRC = GIT"]
    SRC_LOCAL["SRC = LOCAL (HUB folder)"]
    SRC_HTTP["SRC = HTTP (download URL)"]

    SCRIPT <--> HUB

    HOST -->|"ref NODE via HUB"| HUB
    HUB -.->|"resolve NODE"| NODE

    NODE -->|"ref EXT via HUB"| HUB
    HUB -.->|"resolve EXT"| EXT

    EXT --> SRC_GIT
    EXT --> SRC_OCI
    EXT --> SRC_LOCAL
    EXT --> SRC_HTTP
```

Supported extension source types are:

| Source type | Meaning |
| --- | --- |
| `git` | Git repository |
| `oci` | OCI artifact |
| `local` | Local folder on HUB |
| `http` | Download from an HTTP or HTTPS URL |

A local source refers to HUB's filesystem, not the user's browser or the assigned
host. HUB MUST make the required source contents available for host preparation.
HTTP identifies the download mechanism; an archive format such as ZIP is not a
separate source type. The existing stub API exposes local, Git, and ZIP sources;
its source contract and frontend MUST be aligned with these four source types.

## Registration, preparation, and execution sequence

```mermaid
sequenceDiagram
    participant USER
    participant HUB
    participant HOST
    participant NET

    Note over NET: Bootstrap HUB

    USER->>HUB: EXT-REG
    HUB-->>USER: OK-EXT
    Note over USER,HUB: Custom node registration

    USER->>HUB: NODE-INFO
    HUB-->>USER: OK

    Note over HUB,HOST: Node EXT

    HUB->>HUB: INIT EXT
    HUB->>HOST: Bootstrap / prepare
    HOST->>HUB: Preparation response
    HUB-->>USER: OK EXT

    Note over HUB,NET: EXEC phase

    HUB->>HOST: INIT NODE
    HOST-->>HUB: NODE OK
    HOST->>HOST: EXEC NODE
```

`NODE OK` acknowledges acceptance of `INIT NODE` only. Execution happens later.
It MUST NOT be interpreted as process startup or successful execution. Success
and failure are always reported through the corresponding Extension or Node CR
status; acknowledgements do not replace those status updates.

## Independent configuration concerns

Keep execution, pipeline type, and plugin settings independent. A custom
transcriber participates in the pipeline as a transcriber; custom execution
determines how Diamant launches it.

| Concern | Required configuration | Responsibility |
| --- | --- | --- |
| Pipeline type | `audio_source`, `transcriber`, `translator`, or `display` | Input compatibility and communication contract |
| Execution | Runtime and entry point | Host process launch |
| Plugin settings | Arbitrary key/value object, which MAY be empty | Component-specific configuration |
| Argument adapter | Ordered bindings from settings and runtime values | CLI argument construction |
| Communication | Contract for the selected type | Frame types, payloads, and connection behavior |

Host assignment, enabled state, input reference, and restart controls remain node
configuration outside plugin settings. Working directory and environment MAY be
provided as execution settings. Omitted working directories MUST resolve to a
documented host default.

Custom settings MUST NOT inherit the built-in implementations' model, provider,
device, or presentation restrictions. Type requirements belong to the protocol
contract rather than a mandatory set of built-in CLI flags.

## Execution

Support two explicit execution modes initially:

- `executable`: launch a host-installed executable directly.
- `python`: launch a Python script or module through Diamant's Python launcher.

Diamant MUST preserve the existing process supervision behavior: output
forwarding, exit reporting, restart handling, graceful termination, and descendant
cleanup. Python MUST NOT be inserted into executable-mode commands.

Executable paths and Python entry points MUST resolve to components available on
the assigned host before launch. An extension can install those components and
prepare their prerequisites; manual installation is not required by this model.
Installation failures MUST be reported separately from node launch failures.

## Declarative CLI adapter

The adapter translates configuration into an ordered argument array. It MUST NOT
construct a shell command or evaluate shell expressions.

Example extension adapter specification for a custom transcriber. Settings
declare defaults or whether an instance value is required:

```yaml
type: transcriber
execution:
  runtime: executable
  executable: /opt/plugins/my-transcriber

settings:
  model:
    default: fast
  threads:
    default: 4
  useGpu:
    default: true
  language: {}
  device:
    required: false

arguments:
  - flag: --server
    source: runtime.websocketUrl
  - flag: --model-name
    source: settings.model
  - flag: --workers
    source: settings.threads
  - flag: --gpu
    source: settings.useGpu
    mode: switch
  - flag: --language
    source: settings.language
  - flag: --device
    source: settings.device
```

With a resolved endpoint of `wss://proxy/transcribe`, an instance language of
`en`, and no device value, the argument array is:

```json
[
  "--server", "wss://proxy/transcribe",
  "--model-name", "fast",
  "--workers", "4",
  "--gpu",
  "--language", "en"
]
```

Settings have three cases:

| Declaration | Behavior when the node omits the setting |
| --- | --- |
| `default` provided | Use the default; the node MAY override it |
| No default and `required` omitted or `true` | Reject the missing required value |
| No default and `required: false` | Leave the setting absent and omit its argument binding |

An absent optional setting MUST omit both its flag and value. Explicit null,
`false`, zero, and empty strings MUST NOT be treated as absence. In particular,
checking for a default MUST use field presence rather than its truthiness.

The adapter SHOULD support:

- Literal arguments and positional values.
- Separate flag/value arguments and joined `--flag=value` arguments.
- Boolean switches and explicit boolean values.
- Repeated arguments for lists, preserving list order.
- Required bindings, defaults, and omission of absent optional values.
- Explicit JSON serialization for objects and other structured values.

Settings MAY contain arbitrary JSON values. Each binding MUST define an
unambiguous conversion to arguments. Objects MUST require explicit serialization;
they MUST NOT silently become strings. Missing values MUST remain distinguishable
from `false`, zero, and empty strings. Boolean switch mode emits the flag for
`true` and omits it for `false`.

Bindings MUST preserve their configured order. Unknown references, missing
required values, and incompatible value types MUST fail validation before launch.
Values containing spaces or shell characters MUST remain individual arguments.

Diamant supplies a reserved runtime context independently of plugin settings.
Initially, it MUST provide the full type-specific `runtime.websocketUrl`,
including the `ws` or `wss` scheme and endpoint path. Plugin settings MUST NOT
override runtime values.

Every runtime value required by the type contract MUST have a declared delivery
binding. CLI arguments are the primary mechanism; environment bindings MAY also
be supported. A plugin MAY choose its own flag or environment variable names.

The adapter maps configuration only. A component using a different communication
protocol requires a wrapper that implements the Suntitle type contract.

## WebSocket contract reference

Plugins act as WebSocket clients and connect to proxy endpoints. They do not
need to host WebSocket servers.

| Type | Proxy endpoint | Plugin receives | Plugin sends |
| --- | --- | --- | --- |
| Audio source | `/` | No required application input | Binary audio frames |
| Transcriber | `/transcribe` | Binary audio frames | UTF-8 transcription text |
| Translator | `/translate` | UTF-8 transcription text | UTF-8 translated text |
| Display | `/display` | Subtitle text | No required application output |

Publish a readable reference for each supported type. It
MUST specify:

- Connection direction and the resolved endpoint supplied by Diamant.
- Required send and receive operations.
- WebSocket frame types and application message boundaries.
- Payload format, including audio encoding and sample rate where applicable.
- Supported control messages and close behavior.
- Reconnection responsibilities and shutdown expectations.
- Example exchanges suitable for implementing a plugin in any language.

The reference MUST describe and test actual supported behavior. The extension
specification does not declare a protocol version.

The node configuration UI SHOULD show the selected type's contract and how its
required runtime values map to the plugin's arguments. Any UI design change
requires a shadcn/ui proposal in the Suntitle Figma file and explicit approval
before implementation, following the project workflow.

## Implementation sequence

1. Finalize the extension adapter specification, argument binding model, and
   type contracts. Define node references to reusable extensions and instance
   setting overrides. Decide the concrete API and Kubernetes representation while
   retaining independent type and execution concerns.
2. Extend API models, node-type metadata, Kubernetes schemas, validation, and mock
   fixtures. Add the Extension custom resource backing the stub Extensions API
   and controller reconciliation logic. Align source types with Git, OCI, local
   HUB folders, and HTTP downloads.
   Validate input compatibility using the selected pipeline type.
3. Implement extension source handling, plugin discovery, installation on assigned
   hosts, prerequisite preparation, and installation status reporting.
4. Extend Diamant command construction with direct executable and explicit Python
   execution paths, runtime context resolution, and declarative argument binding.
5. Publish the type-specific WebSocket reference and verify example exchanges.
6. Propose the extension and node configuration UI in Figma. After approval,
   implement it with shadcn/ui and update relevant Cypress coverage.

## Validation

- Verify extension source handling, plugin discovery, and prerequisite preparation
  on the assigned host. Failed installation MUST prevent launch and expose an
  actionable installation error.
- Verify Git, OCI, local HUB folder, and HTTP download sources. Local paths MUST
  resolve on HUB, and downloaded archive formats MUST NOT be source types.
- Verify multiple nodes can reference a reusable extension while keeping
  their setting values independent.
- Verify built-in nodes need no extension reference and custom nodes require one.
- Verify defaulted, required without a default, and optional without a default
  settings, including omission of both flag and value for absent optional settings.
- Verify `NODE OK` reports acceptance only and subsequent preparation and execution
  results are recorded on the corresponding CR status.
- Verify direct executable launch and explicit Python launch independently.
- Verify ordered arguments, literal values, spaces, boolean modes, repeated
  values, defaults, optional omission, and explicit JSON serialization.
- Reject unknown references, missing required bindings, and invalid conversions.
- Verify full WebSocket URL delivery, including `wss` and endpoint paths.
- Exercise each type's documented send/receive contract with a custom component.
- Verify process exit reporting, restart, graceful shutdown, and descendant
  cleanup using the existing Diamant supervisor.
- Run relevant Rust tests and required formatting and lint checks when code is
  implemented. Run relevant Cypress tests after UI changes.

## Initial scope

The first version covers extension sources and host prerequisite installation,
reusable extension adapter specifications, explicit runtime selection, arbitrary plugin
settings, declarative CLI mapping, and the existing four types. Protocol
conversion remains a separate capability.

## Existing code

- [Diamant process supervision and command construction](../ws-proxy/src/diamant/nodes.rs)
- [Node resources and settings](../ws-proxy/src/kubernetes/resources.rs)
- [Node-type catalog](../ws-proxy/src/api/node_types.rs)
- [WebSocket proxy routes](../ws-proxy/src/main.rs)
- [Extension API contracts and placeholder handlers](../ws-proxy/src/api/extensions.rs)
- [Extensions frontend](../ui/src/pages/extensions-page.tsx)

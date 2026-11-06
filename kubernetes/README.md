# Kubernetes resources and Control API

Phase 2 switches Control API resource handlers to Kubernetes. The selected API is
`control.suntitle.io/v1alpha1`, kinds `SuntitleHost`, `SuntitleNode`, and `SuntitleExtension`. The proxy,
Diamant, and UI integration remain in later phases. Node deletion is phase 4.

From the repository root, with the existing Compose Kine/API server running:

```sh
bash scripts/generate-kubeconfigs.sh
cargo run -p control-api --bin control_api -- generate-crds > kubernetes/crds.yaml
cargo run -p control-api --bin control_api -- bootstrap-kubernetes --kubeconfig pki/admin.kubeconfig
```

Generation needs no database or Kubernetes connection. Bootstrap applies the
namespace, generated CRDs, and separate certificate-user Roles/RoleBindings,
waiting up to 60 seconds per CRD for `Established`. It is repeatable and does not
force server-side apply ownership conflicts. `--namespace` selects another
namespace. Checked-in CRDs are generated from `src/kubernetes/resources.rs`.

Start the Control API with its own certificate identity:

```sh
cargo run -p control-api --bin control_api -- --kubeconfig pki/suntitle-api.kubeconfig
```

`SUNTITLE_API_KUBECONFIG` also selects the credentials; `--namespace` or
`SUNTITLE_KUBERNETES_NAMESPACE` selects the managed namespace (default `suntitle`).
The API does not initialize PostgreSQL. Startup and `GET /readyz` require all three
CRDs to be established and all three resource collections to be readable. Bootstrap
installs a ClusterRole allowing only GET on these three CRDs for readiness.
`--bootstrap-kubeconfig` remains an optional, separate CRD installation identity.

Host/node list responses contain `items` and an opaque `resourceVersion`.
Node GET/create/update/restart responses contain Kubernetes resources. Create
returns 201 and Location, using the path host for `hostRef`. PUT
accepts `{spec: ...}` and replaces only the complete desired spec. Restart has no
request body and patches only a fresh restart token, returning 202. Neither write
uses a resource-version precondition or a preliminary resource read. Metadata and
status are preserved by these patches.

Extension list responses also contain `items` and `resourceVersion`.
`GET /api/v1/extensions/{name}` returns the stored resource, including status.
`POST /api/v1/extensions` accepts `{name?, spec}` and returns 201 with Location.
The optional `name` is the Kubernetes name; `spec.name` is the display name.
Omitting `name` generates an `extension-<UUID>` name. The required spec fields
are `name`, `sourceType` (`local`, `git`, `oci`, or `http`), `source`, and `adapter`.
`adapter.type` selects the pipeline type. `adapter.execution` remains an extensible
object. `adapter.settings` maps setting names to `{default?, required?}` declarations;
`required` defaults to false. A declared default supplies a value without requiring
`required: true`. `adapter.arguments` contains bindings with `source`,
optional `flag`, and `mode` (`value` by default, or `switch`). Switch bindings
require a flag. Sources are `settings.<key>` or `runtime.websocketUrl`.
Execution resolution uses node overrides before extension defaults and reports
missing required settings. Null, false, zero, and empty strings count as supplied
values. Optional settings without values omit their mappings. Neither the Node CR
nor the Extension CR is changed by resolution. Host dispatch and command conversion
remain pending.

`POST /api/v1/extensions/{name}/refresh` patches a fresh `spec.refreshToken` and
returns 202. It preserves metadata and publication status; compare
`status.observedGeneration` to `metadata.generation` before treating status as
current. Preparation and installation are separate controller work.
The Extensions frontend and its legacy mock endpoints still use the older package
contract; migrating them is a separate UI task. Resource API examples are in
`mockserver/fixtures/kubernetes/extension*.json`.

Kubernetes schemas and CEL validate desired fields and immutable changes. The
API does not check input references or deletion timestamps.
The controller owns reference reconciliation and all finalizer management.
API-server errors retain their HTTP status and reason/message/details in the
error envelope. The client retry layer is disabled so transient server errors
also propagate immediately. Transport failures without an API-server response return 503.

Run the phase 2 API suite after bootstrap:

```sh
cargo test -p control-api --test control_api_resources -- --include-ignored
```

It uses unique resources, exercises actual API handlers with fresh in-memory API
state for every request, and explicitly cleans up finalizers and resources.
Fault-injection tests cover upstream error propagation and readiness failures.

`pki/suntitle-api.kubeconfig` and `pki/suntitle-proxy.kubeconfig` use distinct
certificate CNs. Neither identity can install CRDs or RBAC. API reads hosts and
writes node desired state, but cannot write status or host finalizers. Proxy can
watch/manage resources and status and perform lifecycle cleanup. Nodes reference hosts through `spec.hostRef.name`. A dedicated GC controller
deletes referencing nodes on host deletion and retains the host cleanup finalizer
until their runtime cleanup finishes.

Specs reuse the current typed Rust settings through an externally tagged enum:

```yaml
settings:
  transcriber:
    model: small
    language: en
```

Exactly one key (`audio_source`, `transcriber`, `translator`, or `display`) selects
the node type. There is no separate `spec.type`. Each settings structure owns its
field validation; the settings field enforces type immutability.

`JsonSchema` derives generate field schemas; specs/fields with CEL rules use `KubeSchema` and `x_kube(validation)`
as in kube's `crd_derive_cel` example. `kube/cel` also provides local
`validate_cel` / `validate_cel_update` methods. Schema validation accepts only
`pcm_s16le`, supported Whisper models, lowercase 2–3 character language codes,
and the `huggingface` translator provider. Settings belonging to another type
are rejected. `enabled` defaults to true. `inputRef`, `description`, `restartToken`
and device/model options may be absent or null. Kubernetes applies the Rust
defaults within the selected variant: `task=transcribe`,
`beamSize=5`, `vadEnabled=true`, `preserveFormatting=true`, and `theme=dark`.
Unknown fields follow Kubernetes pruning; use `kubectl --validate=strict` to reject typos. Host reference
and node type are immutable. Only `status.reportedMetadata` preserves arbitrary
JSON. Status is written through `/status`; condition lists merge by `type`.

Run the live acceptance suite after bootstrap:

```sh
python3 scripts/test-kubernetes-resources.py
```

The suite uses the three explicit local kubeconfigs and unique resource names;
it removes its own resources and finalizers. It does not delete namespaces or
rely on a garbage collector. Runtime cleanup is handled by controllers, not CRD schemas.

For containers, credentials can come from a shared read-only volume. Generate
kubeconfigs with certificate paths matching their container mount points and use
`https://kube-apiserver:6443` on the Compose network. The Kubernetes API server
continues to use certificate flags, not a client kubeconfig. Prefer mounting only
each application's client files plus the CA; do not expose the admin key or the
other application's key through a whole-directory mount. Containerizing these
application binaries remains a later deployment step.

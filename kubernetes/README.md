# Phase 1 Kubernetes resources

This is an additive contract/bootstrap implementation. PostgreSQL storage, REST
handlers, WebSocket messages and Diamant continue to work as before. No controller
or new resource CRUD handlers run yet. The selected API is
`control.suntitle.io/v1alpha1`, kinds `SuntitleHost` and `SuntitleNode`.

From the repository root, with the existing Compose Kine/API server running:

```sh
bash scripts/generate-kubeconfigs.sh
cargo run --manifest-path ws-proxy/Cargo.toml --bin control_api -- generate-crds > kubernetes/crds.yaml
cargo run --manifest-path ws-proxy/Cargo.toml --bin control_api -- bootstrap-kubernetes --kubeconfig pki/admin.kubeconfig
```

Generation needs no database or Kubernetes connection. Bootstrap applies the
namespace, generated CRDs, and separate certificate-user Roles/RoleBindings,
waiting up to 60 seconds per CRD for `Established`. It is repeatable and does not
force server-side apply ownership conflicts. `--namespace` selects another
namespace. Checked-in CRDs are generated from `src/kubernetes/resources.rs`.

Optionally start the existing API with
`--bootstrap-kubeconfig pki/admin.kubeconfig` (or
`SUNTITLE_BOOTSTRAP_KUBECONFIG`) to apply and await CRDs before normal PostgreSQL
initialization. This option requires bootstrap privileges and fails startup on
installation failure. It does not install RBAC or switch storage. Default API
startup has no Kubernetes dependency. Bootstrap credentials must remain separate
from the API/proxy runtime credentials; startup installation is a local opt-in.

`pki/suntitle-api.kubeconfig` and `pki/suntitle-proxy.kubeconfig` use distinct
certificate CNs. Neither identity can install CRDs or RBAC. API can read hosts,
maintain host finalizers, and CRUD node specs, but cannot write status. Proxy can
watch/manage resources and status and perform lifecycle cleanup. Owner finalizer
permissions allow `blockOwnerDeletion: true`. Kubernetes RBAC cannot restrict
host patch permission to one metadata field; host-spec ownership is a controller
contract, not field-level authorization.

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
rely on a garbage collector. Resource ownership/reference agreement and runtime
cleanup are contracts for the later controller, not implemented by CRD schemas.

For containers, credentials can come from a shared read-only volume. Generate
kubeconfigs with certificate paths matching their container mount points and use
`https://kube-apiserver:6443` on the Compose network. The Kubernetes API server
continues to use certificate flags, not a client kubeconfig. Prefer mounting only
each application's client files plus the CA; do not expose the admin key or the
other application's key through a whole-directory mount. Containerizing these
application binaries remains a later deployment step.

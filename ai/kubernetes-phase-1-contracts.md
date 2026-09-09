# Kubernetes phase 1 contracts

These contracts define the Kubernetes replacement. Phase 2 implements the REST
resource operations except deletion. WebSocket/controller behavior remains for
phase 3, deletion for phase 4, and UI integration for phase 5.

## Resource and field ownership

API group/version: `control.suntitle.io/v1alpha1`, namespaced hosts and nodes.
Address resources by namespace/name; use server-assigned UID for incarnation
identity; resourceVersion is an opaque server/watch token. Node spec contains display
name, optional description, immutable hostRef/name, enabled (default true),
optional inputRef/name, externally tagged settings and optional restartToken.
The single settings key selects the immutable node type; there is no spec.type. Host spec
contains its display name. CRD schemas are authoritative for local validation.

Kubernetes owns metadata UID, generation, resourceVersion and creationTimestamp.
API owns node desired spec. Proxy registration creates hosts; users/API own host
display configuration. Proxy owns status, sessions, conditions, owner-reference
maintenance and cleanup. The controller establishes host and node cleanup
finalizers before runtime dispatch. API creation sets the host controller owner;
reference and ownership reconciliation belongs to the controller. The API does
not check deletion timestamps or perform cross-resource consistency validation.

Host status contains ip, agentVersion, reportedMetadata, lastHeartbeat, sessionId,
presence and conditions. Node status contains phase, message, observedGeneration,
acknowledgedRestartToken, reportingSession and conditions. Updated=False is
persisted for a pending generation; Updated=True requires runtime acknowledgement
(host configuration only needs controller acceptance). lastTransitionTime changes
only on condition status transitions. Top-level observedGeneration remains the
last successfully applied generation. Status changes do not trigger config apply.

## Future REST contract

Base `/api/v1`, managed namespace configured by the server. GET `/hosts` and
`/nodes` return `{items: [resource], resourceVersion: string}`; GET `/nodes/{name}`
returns `{apiVersion, kind, metadata, spec, status?}`. Responses contain resource
metadata rather than legacy IDs or embedded host snapshots.

POST `/hosts/{hostName}/nodes`: `{name?: string, spec: SuntitleNodeSpec}`. The path
selects `spec.hostRef.name`. Server allocates a metadata.name when omitted, fetches
the host, and sets its controller owner reference. Returns 201 resource and Location
after persistence, without implying execution. No API-managed finalizers.

PUT `/nodes/{name}`: `{spec: SuntitleNodeSpec}` replaces the complete desired spec
without a preliminary read or resource-version precondition. Returns 200 resource;
metadata and status are preserved. Schema/CEL enforce immutable type and hostRef.

POST `/nodes/{name}/restart`: no request body. Patches a fresh opaque restartToken
and returns 202 resource after persistence. No resource-version check. Multiple
unobserved requests may coalesce; completed tokens must not restart again.
DELETE `/nodes/{name}`: `{resourceVersion: string, uid: string}` uses Kubernetes
delete preconditions and returns 202 resource, visible while terminating. Deletion
finishes only after matching runtime cleanup acknowledgement. No offline timeout
releases finalizers. GET after completion returns 404.

Error envelope: `{error: {code: string, message: string, details?: object}}`.
API-server errors retain their HTTP status code, with their reason in `error.code`
and their message/details preserved. There is no API consistency-error remapping
or retry layer. Malformed JSON returns 400 InvalidRequest. Transport failures
without an API-server response and unestablished readiness return 503 Unavailable.
Authentication retains 401 Unauthorized / 403 Forbidden.

## Future WebSocket control protocol

JSON envelopes use `protocolVersion: 1` and `type` discriminator. Resource names
are namespace-local. Opaque IDs/tokens are strings; generations are integers.
Audio/transcription/translation relay traffic is unaffected.

| type | Required payload |
| --- | --- |
| register | hostName, agentVersion, reportedMetadata; optional ip |
| registered | host: {namespace,name,uid}, sessionId |
| heartbeat | hostUid, sessionId |
| apply | target: {namespace,name,uid}, hostUid, sessionId, generation, restartToken (nullable), operationId, spec (complete SuntitleNodeSpec) |
| remove | target: {namespace,name,uid}, hostUid, sessionId, generation, operationId |
| acknowledgement | target: {namespace,name,uid}, hostUid, sessionId, generation, operationId, operation (apply/remove), restartToken (nullable), success (boolean), message (nullable) |

Proxy assigns a fresh session per registration and operationId per logical work
item. Retries reuse that operationId; newer desired generations supersede pending
apply work. Remove has a distinct operation identity and cannot be completed by
an apply acknowledgement. Runtime remembers completed operations within a
session and returns the same result for duplicate delivery. Apply sends complete
desired state, not patches; reapplying an already applied generation/token does
not restart it. A fresh session resynchronizes all current desired resources.
Restart tokens already acknowledged must remain distinguishable across reconnects.

Acknowledgements must match namespace/name/UID, host UID, current session,
generation, operation and operationId (and restart token for apply). Stale or
replaced-resource reports do not update status or release finalizers. Failed
removal retains the finalizer and retries; offline deletion waits for reconnect.
Host deletion explicitly deletes children indexed by host owner UID before
releasing its finalizer, since this stack has no garbage-collection controller.

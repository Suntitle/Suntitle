# API and ws-proxy separation

## Design

Run `control_api` on port 3031 and `ws-proxy` on port 3030 as separate processes.
Both use PostgreSQL through a shared `ControlStore` interface. Store hosts, nodes,
and pending commands durably; use LISTEN/NOTIFY to wake the proxy after changes.
Keep WebSocket connections and routing maps in proxy memory.

## Shared models

Use the existing domain models directly in the API and storage. No separate
database models and no changes to their public types or JSON field layout.
Preserve all Serde and OpenAPI annotations.

- `Host` and `HostStatus` remain unchanged.
- `Node` remains the existing tagged enum containing `AudioSourceNode`,
  `TranscriberNode`, `TranslatorNode`, and `DisplayNode`, each with `NodeBase`
  and its typed settings.
- `NodeCreate` continues to wrap `NodeUpdate`. `NodeUpdate` retains
  `pub settings: NodeSettings` with the existing flattened Serde representation.
- `NodeSettings` remains the tagged enum of the four typed settings models.
  Keep `inputNodeId`, required nullable field handling, and runtime validation.

The storage interface returns these current models, including typed
`NodeSettings`, rather than generic JSON maps or adapter-specific response types.
JSONB is the persistence format; it does not change the domain types or their
validation rules.

## Storage and events

Use **SeaQuery + SQLx** for generated queries and `Json<Host>` / `Json<Node>` for
JSONB serialization. Keep database details inside the adapter.

| Table | Stored data |
| --- | --- |
| `hosts` | UUID primary key and `Host` JSONB document |
| `nodes` | UUID primary key, host foreign key, revision, `Node` and `NodeUpdate` JSONB documents |
| `control_commands` | UUID, host/node references, revision, registration message JSONB, delivery state |

Keep `NodeUpdate` because it includes desired input configuration absent from the
node response. Populate embedded node hosts from authoritative host records on read.
Expose typed methods for host/node reads, status, and pending commands. Encapsulate
SQL writes in Create, Update, and Delete operations. The caller first Gets the
current domain model, modifies the required fields, then submits the resulting
model to Update. Update writes the supplied values directly; it does not fetch or
merge fields. A version check rejects stale writes so the caller can repeat Get
and modification before retrying. Commit desired state and outbox changes in one
transaction. Disconnect does not delete hosts or desired nodes; Delete can remove
acknowledged command history without discarding pending work.

- API validates and atomically saves desired state, increments revision, and inserts
  a command. A database trigger calls `pg_notify('control_changes', host_id::text)`.
- Proxy uses `PgListener`, then reads pending commands for connected hosts. Subscribe
  before scanning; rescan after reconnect and periodically to recover missed signals.
- Complete commands after matching terminal host status, not socket writes. Retry
  using revisions and preserve ordering per node. Keep Diamant's existing behavior
  of ignoring duplicate or older revisions. Reconcile fresh agents with latest
  desired state before dispatch. Repeating lost terminal acknowledgements and
  other duplicate-command corner cases are deferred.
- Lock or version document updates to preserve concurrent settings/status changes.
  Disconnect retains desired nodes; expire host health and reject stale sessions.

Start with one proxy instance. Diamant keeps using WebSockets without DB access.

## Local setup

Add root `compose.yaml` for PostgreSQL; keep Rust binaries running on the host:

```yaml
name: suntitle
services:
  postgres:
    image: postgres:18
    environment:
      POSTGRES_DB: suntitle
      POSTGRES_USER: suntitle
      POSTGRES_PASSWORD: suntitle-local
    ports:
      - "127.0.0.1:5432:5432"
    volumes:
      - postgres-data:/var/lib/postgresql
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U suntitle -d suntitle"]
      interval: 2s
      timeout: 3s
      retries: 30
volumes:
  postgres-data:
```

These credentials are for local development. Add root `.env.example`, copied to
ignored `.env.local`, with:

```dotenv
DATABASE_URL=postgres://suntitle:suntitle-local@127.0.0.1:5432/suntitle
```

A shared configuration loader reads `.env.local` without overriding process environment.
Both binaries create a bounded SQLx pool and inject `PostgresStore` into their handlers;
create the API pool outside the Actix worker factory. Only proxy starts the listener.
Missing configuration, unavailable storage, or incompatible schema fails startup.

Update `justfile`:

- `db-up`: `docker compose up -d --wait --wait-timeout 90 postgres`.
- `db-migrate`: run a Rust migration binary using `DATABASE_URL` and versioned migrations.
- `db-stop`: stop PostgreSQL while preserving its volume.
- `api` / `proxy`: run `control_api` / `ws-proxy` independently.
- `local-stack`: prepare environment, start DB, migrate, start API/proxy, wait for
  both, then start Diamant/UI. Ctrl+C stops application children; DB stays available.
- `ui`: start DB, migrate, then run API and UI.

Document these commands and a short backup/restore procedure in `README.md`.

## Implementation and checks

1. Preserve the existing `Node`, `NodeUpdate`, and `NodeSettings` models, request
   validation, helpers, and OpenAPI; verify their JSONB round trips.
2. Add shared storage, migrations, command notifications, and local configuration.
3. Wire API and proxy to storage; remove the HTTP server from proxy and local
   host/node registries as the source of truth.
4. Update local recipes and E2E examples to launch both binaries and PostgreSQL.
5. Test JSONB round trips, concurrent updates, commit/rollback notifications,
   missed-event recovery, duplicate commands, restart replay, and persistence across
   disconnects and restarts. Run Rust tests, formatting, clippy, and the two E2E examples.

Feasibility: verify typed domain-model serialization and fixture compatibility;
live PostgreSQL integration is required.
Verified library combination: SQLx 0.8.6, SeaQuery 0.32.7, sea-query-binder 0.7.0.
Follow AGENTS.md for ticket handling and approval before committing.

References: [SQLx JSON](https://docs.rs/sqlx/0.8.6/sqlx/types/struct.Json.html),
[PgListener](https://docs.rs/sqlx/0.8.6/sqlx/postgres/struct.PgListener.html),
[PostgreSQL image](https://hub.docker.com/_/postgres).

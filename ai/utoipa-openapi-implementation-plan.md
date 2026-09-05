# Utoipa OpenAPI implementation plan

## Goal

Create a code-first Actix Web API in `ws-proxy` whose Utoipa-generated OpenAPI document is written to [`specs/openapi.yaml`](../specs/openapi.yaml).

The first implementation is contract scaffolding only:

- Define every data model from the current specification.
- Register every operation from the current specification as an Actix Web route.
- Give every handler a concrete, contract-correct signature, but leave its body as `todo!()`.
- Treat the Rust declarations and route annotations as the source of truth for `specs/openapi.yaml`.
- Prefer idiomatic Utoipa output over exact structural parity with the previous hand-written document.

## Non-goals

- Do not implement authentication, persistence, validation behavior, node lifecycle operations, or other business logic.
- Do not connect these REST routes to the existing WebSocket forwarding pipeline.
- Do not add Swagger UI or another documentation UI.

## Proposed dependencies

Add compatible current versions of:

```toml
actix-web = "4"
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
utoipa = { version = "5", features = ["actix_extras", "chrono", "uuid", "yaml"] }
utoipa-actix-web = "0.1"
uuid = { version = "1", features = ["serde", "v4"] }
```

Resolve versions through Cargo rather than copying the illustrative versions blindly. Preserve unrelated existing `Cargo.toml` and `Cargo.lock` changes.

## Target module layout

```text
ws-proxy/src/
  api/
    mod.rs              # ApiDoc metadata, modifiers, and route registration
    auth.rs             # login, logout, getCurrentUser
    hosts.rs            # listHosts
    node_types.rs       # listNodeTypes
    nodes.rs            # install/list/get/update/restart node
    models.rs           # request, response, and component data types
    errors.rs           # Problem, ValidationProblem, and response declarations
  bin/
    generate_openapi.rs # writes specs/openapi.yaml
```

Keep WebSocket behavior separate from this REST contract scaffolding. If sharing the current binary makes the generator awkward, move reusable API declarations into `ws-proxy/src/lib.rs` and let both binaries import the library.

## Phase 1: establish repeatable generation

1. Add a generator binary that builds `ApiDoc::openapi()`, serializes it as YAML, and writes `../specs/openapi.yaml` relative to the `ws-proxy` manifest directory.
2. Make output deterministic: use stable component/path ordering and always end the file with one newline.
3. Use one documented command for the loop:

   ```bash
   cargo run --manifest-path ws-proxy/Cargo.toml --bin generate_openapi
   ```

## Phase 2: model the components first

Implement serializable Rust types and Utoipa schemas for every named schema before adding handlers.

### Foundation and error models

- `LoginRequest`, `LoginResponse`, `User`
- `Problem`, `ValidationProblem`, and the validation error item
- Shared component responses: `BadRequest`, `Unauthorized`, `NotFound`, and `ValidationFailed`
- Bearer JWT security scheme named `bearerAuth`

### Host and common enum models

- `Host`, `HostList`, `HostStatus`
- `NodeType`, `NodeStatus`, `ConnectionStatus`, `DataType`
- Preserve UUID, IP, date-time, URI-reference, password, ranges, lengths, patterns, examples, required fields, and `additionalProperties` behavior.

### Node-setting metadata models

- `NodeSettingValue`
- `NodeSettingOption`
- `NodeSettingDefinition`
- `NodeTypeDefinition`, `NodeTypeDefinitionList`
- `LanguageTag`

Represent `NodeSettingValue` as an untagged enum supporting string, number, boolean, and null.

### Concrete settings models

- `AudioSourceSettings`
- `TranscriberSettings`
- `TranslatorSettings`
- `DisplaySettings`

Match every default, enum value, numeric boundary, nullable field, example, and `additionalProperties: false` declaration.

### Node union models

- `NodeBase`, `NodeConnection`
- `AudioSourceVariant`, `TranscriberVariant`, `TranslatorVariant`, `DisplayVariant`
- `AudioSourceNode`, `TranscriberNode`, `TranslatorNode`, `DisplayNode`, and `Node`
- `NodeUpdateBase`, `NodeInputUpdate`
- All concrete create and update types
- `NodeCreate`, `NodeUpdate`, and `NodeList`

Preserve the model semantics and named components using derives and schema attributes. Prefer idiomatic Utoipa references and composition; use a narrowly scoped `Modify` implementation only for document behavior that derives cannot express.

After each model group, regenerate and compare only `components.schemas` so schema differences are isolated before route work begins.

## Phase 3: add all Actix routes with `todo!()` handlers

Mount the API under `/api/v1`, matching the established server URL. Use Actix attribute macros plus `#[utoipa::path]`, and register handlers through Utoipa Actix `.service(...)` calls so route registration and documentation collection stay coupled.

Add these ten operations:

| Method | Path | Rust handler / operation ID |
| --- | --- | --- |
| POST | `/auth/login` | `login` |
| POST | `/auth/logout` | `logout` |
| GET | `/auth/me` | `get_current_user` / `getCurrentUser` |
| GET | `/hosts` | `list_hosts` / `listHosts` |
| POST | `/hosts/{hostId}/nodes` | `install_node_on_host` / `installNodeOnHost` |
| GET | `/node-types` | `list_node_types` / `listNodeTypes` |
| GET | `/nodes` | `list_nodes` / `listNodes` |
| GET | `/nodes/{nodeId}` | `get_node` / `getNode` |
| PUT | `/nodes/{nodeId}` | `update_node` / `updateNode` |
| POST | `/nodes/{nodeId}/restart` | `restart_node` / `restartNode` |

For each handler:

1. Use typed Actix extractors for path, query, and JSON body values.
2. Use concrete response or error types so the Rust signature represents the contract.
3. Explicitly document every established response status, media type, description, header, and component reference.
4. Set the exact tag, summary, description, operation ID, and security override.
5. Leave only the implementation body unfinished:

   ```rust
   async fn list_nodes(query: web::Query<ListNodesQuery>) -> ApiResult<NodeList> {
       todo!()
   }
   ```

The application must compile and start. Calling a scaffolded endpoint may panic because its body is intentionally `todo!()`; that behavior is temporary and should be stated in the binary log or README.

## Phase 4: reproduce document-level metadata

Configure Utoipa to emit the established document metadata:

- OpenAPI `3.1.0`
- title `Suntitle Control API`
- version `0.1.0`
- the complete API description
- server URL `/api/v1` and its description
- tags and their display names/order
- global bearer security, with `POST /auth/login` explicitly unsecured
- named reusable responses and the `WWW-Authenticate` header/example

Do this in `ApiDoc` attributes where supported and in one documented `Modify` implementation for details that attributes cannot represent.

## Phase 5: verify generated output

Generate `specs/openapi.yaml`, inspect it for the expected paths and named schemas, then run
`cargo fmt`, `cargo clippy`, and the full `ws-proxy` test suite.

## Acceptance criteria

- `ws-proxy` compiles with all ten REST handlers present and registered.
- Every REST handler body remains `todo!()`; no business logic is introduced.
- All schemas and reusable responses from `specs/openapi.yaml` are represented by Rust/Utoipa declarations.
- `specs/openapi.yaml` is generated solely from Rust declarations and route annotations.
- Formatting, linting, and relevant tests pass.
- No commit is created until the user reviews and explicitly approves the implementation.

## Expected implementation risks

- Utoipa may inline a schema where the previous document used a named `$ref`, or generate a tagged enum in a different but semantically similar shape. Prefer the idiomatic generated representation unless it changes the intended wire model.
- OpenAPI 3.1 features such as `const`, nullable type arrays, and `unevaluatedProperties` may need explicit schema construction.
- Actix/Utoipa automatic collection only covers supported `.service(...)` registration patterns. Avoid manual `.route(...)` registration for these operations.
- The current `ws-proxy` code is a Warp WebSocket service. Introducing the REST scaffolding must not silently change its existing WebSocket endpoints or state flow.

---
name: update-api
description: Add or revise Suntitle Control API contracts from a user-provided OpenAPI specification using Rust, Actix Web, and Utoipa, while preparing mock data-plane fixtures for separate implementation. Use for contract-first endpoint or model changes; do not use to implement endpoint business logic by itself.
---

# Update API

Update the Suntitle Control API contract from an OpenAPI specification supplied by the user. Rust declarations and Utoipa annotations are the source of truth; `specs/openapi.yaml` is generated output.

## Required input

Require the user to provide the desired OpenAPI contract or fragment. It must identify enough of the operation and schemas to implement the contract, including the method and path, parameters or request body, responses, and relevant security or metadata. If that contract is absent or materially ambiguous, ask for it instead of inventing wire behavior.

Compare the supplied contract with the current generated `specs/openapi.yaml` and classify the work as:

- a new endpoint;
- a change to an existing endpoint; or
- a shared-model change affecting one or more endpoints.

## Contract implementation

Translate the requested contract into the existing code-first structure under `ws-proxy/src/api`:

- Define or update serializable Rust request, response, parameter, error, and component types with appropriate Utoipa derives and schema attributes.
- Preserve formats, required and nullable semantics, defaults, enum values, bounds, lengths, patterns, examples, media types, headers, reusable responses, security, and `additionalProperties` behavior where the supplied contract specifies them.
- Prefer idiomatic Utoipa output over hand-editing generated YAML. Use a narrowly scoped `Modify` implementation only for document behavior that Utoipa attributes or derives cannot express.
- Register schemas, responses, and paths in `ApiDoc` and register Actix handlers through Utoipa Actix `.service(...)` calls. Keep the `/api/v1` scope and existing document metadata unless the supplied contract changes them.
- Keep the WebSocket forwarding pipeline and unrelated API behavior unchanged.

For a new endpoint, add a typed Actix handler whose extractors and return type represent the contract, but leave its body as `todo!()`. The crate and application must still compile and start; invocation of that scaffolded endpoint may panic until business logic is implemented separately.

For an existing endpoint or model, update the contract declarations without inventing new business behavior. Preserve compatible handler logic. If the changed contract makes existing logic incomplete, limit production-code edits to safe compile-time adaptation and clearly report the remaining implementation work; do not pretend the endpoint supports behavior that was only added to the contract.

## Prepare the mock data plane

Every API contract change must leave the mock data plane ready for downstream implementation and UI work:

- Add or update representative JSON fixtures under `mockserver/api/v1` for every affected successful response and changed model.
- Keep fixtures valid against the new contract, including required fields, discriminator or enum values, nesting, and nullability.
- Update other existing fixtures that embed a changed shared model so they do not retain the old shape.
- Ensure `mockserver/server.py` can serve a representative URL for each new operation. Extend its method handling or path resolution only when the current file-based behavior is insufficient.
- Do not build the production business logic or UI consumer unless the user separately requests it.

Use deterministic, realistic mock values. Avoid adding fixture variants that do not exercise a changed field or operation.

## Generate and verify

Regenerate the checked-in definition with:

```bash
cargo run --manifest-path ws-proxy/Cargo.toml --bin generate_openapi
```

Verify that the generated document semantically contains the supplied paths, operations, schemas, responses, and security details. Review the diff to ensure unrelated paths and components did not drift and that the file ends with one newline.

Add or update focused Rust tests for document registration and contract-relevant behavior. Validate changed mock JSON and, when an existing test or consumer uses an affected fixture, update the relevant tests. Then run:

```bash
cargo fmt --manifest-path ws-proxy/Cargo.toml
cargo clippy --manifest-path ws-proxy/Cargo.toml --all-targets --all-features
cargo test --manifest-path ws-proxy/Cargo.toml
```

Inspect the final diff and preserve unrelated user changes. Present the contract scaffolding, mock-data changes, verification results, and any intentionally unimplemented runtime behavior for review. Follow the repository approval workflow before committing.

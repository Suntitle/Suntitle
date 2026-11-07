# Embed uv's CLI in the Rust binary

## Outcome

Expose `diamant uv <arguments...>` by linking uv's Rust crates into Diamant and
calling the safe async `uv::run` API. The embedded mode should not require a
separately installed uv executable or introduce unsafe blocks in Suntitle code.
This document is a plan only; the API sketch has not been compiled or validated.

## Existing integration

The Rust package is in `ws-proxy/`. `src/bin/diamant.rs` currently accepts
`--uv-bin` / `UV_BIN`, defaulting to `uv`. Node command construction and
supervision live in `src/diamant/nodes.rs`; `examples/fake_uv.rs` supports testing.

Add an early dispatch path for `diamant uv ...`, before normal Diamant argument
validation, logging initialization, or service startup. Preserve arguments as
`OsString`, remove the Diamant wrapper argument, and supply `uv` as argv[0].

## API feasibility and dependencies

1. Select a specific uv release or repository revision that exposes safe
   `uv::run` and `uv::GlobalInitialization`. Verify availability in that exact
   version; current upstream source is not a guarantee for published releases.
2. Pin compatible `uv` and `uv-cli` dependencies from the same upstream release
   or revision and retain `Cargo.lock`. Check compatibility with existing Clap,
   Tokio, Rust toolchain, supported targets, and dependency features.
3. Compile a minimal adapter before integrating it. Conceptual call sequence:

   ```rust
   use clap::Parser;

   let cli = uv_cli::Cli::try_parse_from(args)?;
   let status = uv::run(cli, uv::GlobalInitialization::Initialize).await?;
   let exit_code: std::process::ExitCode = status.into();
   ```

   Run this within an appropriately configured Tokio runtime. Adapt error
   conversion to Suntitle's existing error handling. Enforce
   `#![forbid(unsafe_code)]` in the adapter; this constrains our code, not upstream
   dependencies.
4. If the selected release lacks the safe API, record the blocker and choose a
   compatible revision before implementation; do not silently call unsafe
   `uv::main` instead.

## CLI behavior and process lifetime

`uv::run` bypasses uv's main wrapper. Reproduce the necessary runtime setup,
help/version handling, parse-error exit codes, command exit codes, diagnostics,
and shutdown behavior. Review upstream runtime and stack requirements.

Execute one uv invocation per process. Embedded CLI mode owns that process's
logging and global initialization. Do not invoke it repeatedly or concurrently
inside the long-running Diamant service, where uv's global state and working
directory changes could interfere with application tasks.

Audit wrapper behavior explicitly: upstream `uv::main` sets the `UV` environment
variable to the current executable. A binary requiring an extra `uv` subcommand
cannot be substituted blindly for a plain uv executable. Verify child-process
and recursive invocation behavior and design argument dispatch accordingly.
Avoid process-global environment mutation; where needed, configure child
environments through safe `Command::env` calls.

## Node launcher integration

After CLI mode is validated, use `current_exe()` with an initial `uv` argument as
the default node launcher. This spawns the same compiled binary in embedded uv
mode and preserves process supervision while removing the external uv install
requirement. Keep an explicit `--uv-bin` / `UV_BIN` override for external
executables and existing test fixtures; overrides must receive the original uv
arguments without an extra `uv` prefix.

Preserve working directories, environment, stdout/stderr forwarding, cancellation,
signal handling, exit reporting, and descendant cleanup from the existing node
supervisor. Keep argument-prefix handling explicit in command construction.

## Validation and completion

- Compile the safe adapter against pinned dependencies on supported targets.
- Exercise `diamant uv --help`, `--version`, unknown arguments, a successful
  command, and a failing command; compare output routing and exit codes with the
  matching upstream CLI.
- Test argument forwarding, including spaces and non-UTF-8 arguments where
  supported, plus embedded/default and external-override command construction.
- Run a supervised child through embedded mode and verify cancellation and
  descendant cleanup. Cover commands that rely on the `UV` environment variable.
- Verify normal Diamant startup and existing fake-uv tests still work. Run
  relevant Rust tests, formatting, Clippy, and node-handling E2E checks.
- Document usage, dependency pins, build-size impact, and any remaining CLI
  differences. Claim CLI compatibility only for behavior actually verified.

## References

- [uv implementation and safe run API](https://github.com/astral-sh/uv/blob/main/crates/uv/src/lib.rs)
- [uv binary entry point](https://github.com/astral-sh/uv/blob/main/crates/uv/src/bin/uv.rs)
- [Rust crate versioning policy](https://docs.astral.sh/uv/reference/policies/versioning/#crate-versioning)

The Rust interfaces are internal and unstable. Recheck the adapter and wrapper
behavior on each dependency upgrade.

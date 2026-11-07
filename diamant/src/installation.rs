//! Downloaded payloads become prepared installations after entry-point resolution.
//! A prepared installation waits for argument construction and process launch.
//! Preparation failure MUST NOT publish an installation or start a process.
use std::{
    io,
    path::{Path, PathBuf},
};
use suntitle_core::{kubernetes::adapter::ResolvedAdapter, registration::ApplyIdentity};

/// Shared entry-point contract; prepared file variants contain canonical absolute paths.
pub use suntitle_core::kubernetes::execution::ExecutionCommand;

/// Downloaded files and resolved execution configuration for one custom node revision.
#[derive(Clone, Debug)]
pub struct PreparedExtension {
    /// Resource revision and host session that selected this installation.
    pub identity: ApplyIdentity,
    /// Immutable registry reference used to obtain the payload.
    pub artifact_reference: String,
    /// Installed extension root, including any directory prefix from the artifact.
    pub root: PathBuf,
    /// Existing directory from which the custom process will run.
    pub working_directory: PathBuf,
    /// Existing executable or Python entry point. No command arguments are built yet.
    pub command: ExecutionCommand,
    /// Resolved instance settings and argument bindings supplied by the controller.
    pub adapter: ResolvedAdapter,
}

impl PreparedExtension {
    /// Resolve the restored payload and execution paths without running any program.
    pub async fn prepare(
        identity: ApplyIdentity,
        artifact_reference: String,
        payload: PathBuf,
        adapter: ResolvedAdapter,
    ) -> io::Result<Self> {
        let root = if tokio::fs::metadata(&payload).await?.is_dir() {
            payload
        } else {
            payload
                .parent()
                .ok_or_else(|| io::Error::other("extension file has no installation directory"))?
                .to_owned()
        };
        let root = tokio::fs::canonicalize(root).await?;
        let execution = &adapter.execution;
        let working_directory = match execution.working_directory.as_deref() {
            Some(path) => resolve(&root, path).await?,
            None => root.clone(),
        };
        if !working_directory.is_dir() {
            return Err(io::Error::other(
                "extension workingDirectory MUST be a directory",
            ));
        }
        let command = match &execution.command {
            ExecutionCommand::Executable(executable) => {
                let path = resolve_file(&root, executable).await?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if tokio::fs::metadata(&path).await?.permissions().mode() & 0o111 == 0 {
                        return Err(io::Error::other("extension entry point is not executable"));
                    }
                }
                ExecutionCommand::Executable(path)
            }
            ExecutionCommand::Python(script) => {
                ExecutionCommand::Python(resolve_file(&root, script).await?)
            }
            ExecutionCommand::PythonModule(module) => {
                ExecutionCommand::PythonModule(module.clone())
            }
        };
        Ok(Self {
            identity,
            artifact_reference,
            root,
            working_directory,
            command,
            adapter,
        })
    }
}

/// Resolve relative paths from the installation root; explicit absolute paths remain supported.
async fn resolve(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let resolved = tokio::fs::canonicalize(root.join(path)).await?;
    if path.is_relative() && !resolved.starts_with(root) {
        return Err(io::Error::other(
            "relative execution paths MUST stay inside the extension root",
        ));
    }
    Ok(resolved)
}
/// Require a regular file before the installation can advance to awaiting launch.
async fn resolve_file(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let path = resolve(root, path).await?;
    if !path.is_file() {
        return Err(io::Error::other("extension entry point MUST be a file"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build one dispatched custom node identity.
    fn identity() -> ApplyIdentity {
        serde_json::from_value(json!({"target":{"namespace":"test","name":"custom","uid":"node"},"hostUid":"host","sessionId":"session","generation":1,"operationId":"prepare"})).unwrap()
    }
    /// Build an adapter retaining its configured settings and argument bindings.
    fn adapter(execution: serde_json::Value) -> ResolvedAdapter {
        serde_json::from_value(json!({"nodeRole":"transcriber","execution":execution,"settings":{"language":"cs"},"arguments":[]})).unwrap()
    }

    /// A raw HTTP payload uses its containing directory; script paths use the artifact root.
    #[tokio::test]
    async fn file_payload_and_python_paths_resolve_without_starting_a_process() {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("plugin.py");
        std::fs::write(
            &script,
            "raise RuntimeError('MUST NOT execute during preparation')",
        )
        .unwrap();
        std::fs::create_dir(root.path().join("work")).unwrap();
        let config = adapter(
            json!({"runtime":"python","command":"./plugin.py","workingDirectory":"./work"}),
        );
        let prepared = PreparedExtension::prepare(
            identity(),
            "registry/extensions@sha256:test".into(),
            script.clone(),
            config,
        )
        .await
        .unwrap();
        assert_eq!(
            prepared.working_directory,
            std::fs::canonicalize(root.path().join("work")).unwrap()
        );
        assert!(
            matches!(prepared.command, ExecutionCommand::Python(path) if path == std::fs::canonicalize(&script).unwrap())
        );
        assert_eq!(prepared.adapter.settings["language"], "cs");
        let module = PreparedExtension::prepare(
            identity(),
            "registry/extensions@sha256:test".into(),
            root.path().into(),
            adapter(json!({"runtime":"pythonModule","command":"plugin"})),
        )
        .await
        .unwrap();
        assert!(matches!(module.command, ExecutionCommand::PythonModule(name) if name == "plugin"));
        let missing = PreparedExtension::prepare(
            identity(),
            "registry/extensions@sha256:test".into(),
            root.path().into(),
            adapter(json!({"runtime":"executable","command":"./missing"})),
        )
        .await;
        assert!(missing.is_err());
    }
}

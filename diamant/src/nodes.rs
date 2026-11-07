//! Child-process ownership for nodes assigned to a Diamant host.
//!
//! On Unix, every node is launched in its own process group. Shutdown sends
//! SIGTERM to the complete group, waits for the configured grace period, and
//! then uses SIGKILL as a bounded fallback.
//! Command construction composes URL bindings from the host base and setting-owned
//! endpoint paths immediately before process launch. Setting strings remain unchanged.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::process::{Child, Command};
use url::Url;
#[cfg(test)]
use uuid::Uuid;

use super::installation::PreparedExtension;
use suntitle_core::kubernetes::resources::{NodeSettings, SuntitleNodeSpec};
use suntitle_core::kubernetes::{
    adapter::{ArgumentMode, ArgumentSource, ArgumentValue, RuntimeValue},
    execution::ExecutionCommand,
};
use suntitle_core::registration::{ApplyIdentity, CustomExecution};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRuntimeStatus {
    Running,
    Stopped,
    Failed,
}

impl NodeRuntimeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NodeSnapshot {
    pub name: String,
    pub node_type: &'static str,
    pub status: &'static str,
    pub parameters: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeEvent {
    pub identity: ApplyIdentity,
    pub status: NodeRuntimeStatus,
    pub message: Option<String>,
}

#[derive(Debug)]
struct ChildProcess {
    child: Child,
    #[cfg(unix)]
    process_group: nix::unistd::Pid,
}

impl ChildProcess {
    fn spawn(spec: &CommandSpec) -> io::Result<Self> {
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .current_dir(&spec.current_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }
        let child = command.spawn()?;
        #[cfg(unix)]
        let process_group = nix::unistd::Pid::from_raw(
            child
                .id()
                .ok_or_else(|| io::Error::other("spawned node has no process ID"))?
                .try_into()
                .map_err(|_| io::Error::other("node process ID exceeds i32"))?,
        );
        Ok(Self {
            child,
            #[cfg(unix)]
            process_group,
        })
    }

    fn terminate(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            use nix::errno::Errno;
            use nix::sys::signal::{Signal, killpg};

            match killpg(self.process_group, Signal::SIGTERM) {
                Ok(()) | Err(Errno::ESRCH) => Ok(()),
                Err(error) => Err(io::Error::other(error)),
            }
        }
        #[cfg(not(unix))]
        {
            self.child.start_kill()
        }
    }

    fn force_kill(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            use nix::errno::Errno;
            use nix::sys::signal::{Signal, killpg};

            match killpg(self.process_group, Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => Ok(()),
                Err(error) => Err(io::Error::other(error)),
            }
        }
        #[cfg(not(unix))]
        {
            self.child.start_kill()
        }
    }

    async fn stop(mut self, grace: Duration) -> io::Result<ExitStatus> {
        self.stop_in_place(grace).await
    }

    async fn stop_in_place(&mut self, grace: Duration) -> io::Result<ExitStatus> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(status);
        }
        self.terminate()?;
        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(result) => result,
            Err(_) => {
                self.force_kill()?;
                self.child.wait().await
            }
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.force_kill();
        }
    }
}

#[derive(Debug)]
struct ManagedNode {
    identity: ApplyIdentity,
    spec: SuntitleNodeSpec,
    child: Option<ChildProcess>,
    status: NodeRuntimeStatus,
    message: Option<String>,
    /// Effective command used to detect runnable changes independently of installation.
    command: Option<CommandSpec>,
}

/// Own running node processes and prepared custom installations for one host session.
#[derive(Debug)]
pub struct NodeProcessManager {
    /// Working directory used by built-in implementations.
    project_dir: PathBuf,
    /// Diamant executable providing the embedded Python launcher.
    uv_bin: PathBuf,
    /// Validated data-plane base URL shared by built-in and extension commands.
    data_plane_url: Url,
    /// Time allowed for child processes to terminate before forced cleanup.
    shutdown_grace: Duration,
    /// Current built-in or launched node processes by namespace and name.
    nodes: HashMap<String, ManagedNode>,
    /// Prepared custom payloads waiting for argument construction and launch.
    prepared_extensions: HashMap<String, PreparedExtension>,
}
impl NodeProcessManager {
    /// Validate and normalize the base URL without starting processes or preparing extensions.
    pub fn new(
        project_dir: PathBuf,
        uv_bin: PathBuf,
        data_plane_url: Url,
        shutdown_grace: Duration,
    ) -> Result<Self, String> {
        if !project_dir.is_dir() {
            return Err(format!(
                "project directory does not exist: {}",
                project_dir.display()
            ));
        }
        validate_data_plane_url(&data_plane_url)?;
        let data_plane_url = data_plane_url
            .join("")
            .map_err(|error| format!("invalid data-plane base URL: {error}"))?;
        Ok(Self {
            project_dir,
            uv_bin,
            data_plane_url,
            shutdown_grace,
            nodes: HashMap::new(),
            prepared_extensions: HashMap::new(),
        })
    }

    /// Associate downloaded files with their custom node without starting a process.
    pub fn prepare_extension(&mut self, installation: PreparedExtension) {
        let identity = &installation.identity;
        let key = format!("{}/{}", identity.target.namespace, identity.target.name);
        self.prepared_extensions.insert(key, installation);
    }

    /// Return installed files for this node UID, independently of command revision.
    pub fn prepared_extension(&self, identity: &ApplyIdentity) -> Option<&PreparedExtension> {
        let key = format!("{}/{}", identity.target.namespace, identity.target.name);
        self.prepared_extensions
            .get(&key)
            .filter(|installation| installation.identity.target.uid == identity.target.uid)
    }

    /// Apply built-in or custom execution through one supervisor.
    /// Duplicate and stale operations MUST NOT reconfigure prepared installations.
    /// Custom execution context MUST accompany only custom node settings.
    pub async fn apply(
        &mut self,
        identity: ApplyIdentity,
        spec: SuntitleNodeSpec,
        custom: Option<CustomExecution>,
    ) -> Vec<NodeEvent> {
        let key = format!("{}/{}", identity.target.namespace, identity.target.name);
        if let Some(node) = self.nodes.get_mut(&key) {
            if node.identity == identity {
                return vec![node.event()];
            }
            if node.identity.target.uid == identity.target.uid
                && node.identity.generation > identity.generation
            {
                return Vec::new();
            }
        }
        let command = if let Some(custom) = custom {
            self.prepare_custom_command(&identity, custom).await
        } else {
            command_spec(&self.uv_bin, &self.project_dir, &self.data_plane_url, &spec)
        };
        if let Some(node) = self.nodes.get_mut(&key) {
            if node.identity.target.uid == identity.target.uid
                && node.status != NodeRuntimeStatus::Failed
                && node.spec == spec
                && command
                    .as_ref()
                    .is_ok_and(|command| node.command.as_ref() == Some(command))
            {
                node.identity = identity;
                node.spec = spec;
                return vec![node.event()];
            }
            if let Some(child) = node.child.take()
                && let Err(error) = child.stop(self.shutdown_grace).await
            {
                node.identity = identity;
                node.status = NodeRuntimeStatus::Failed;
                node.message = Some(format!("failed to stop previous process: {error}"));
                return vec![node.event()];
            }
        }
        let selected_command = command.as_ref().ok().cloned();
        let (child, status, message) = if spec.enabled {
            match command.and_then(|command| ChildProcess::spawn(&command)) {
                Ok(child) => (Some(child), NodeRuntimeStatus::Running, None),
                Err(error) => (
                    None,
                    NodeRuntimeStatus::Failed,
                    Some(format!("failed to spawn node: {error}")),
                ),
            }
        } else {
            (None, NodeRuntimeStatus::Stopped, None)
        };
        let node = ManagedNode {
            identity,
            spec,
            child,
            status,
            message,
            command: selected_command,
        };
        let event = node.event();
        self.nodes.insert(key, node);
        vec![event]
    }

    /// Build the current custom command from installed files without downloading again.
    async fn prepare_custom_command(
        &mut self,
        identity: &ApplyIdentity,
        custom: CustomExecution,
    ) -> io::Result<CommandSpec> {
        let installation = self
            .prepared_extension(identity)
            .filter(|installation| installation.artifact_reference == custom.reference)
            .ok_or_else(|| io::Error::other("matching extension artifact is not installed"))?;
        let installation = PreparedExtension::prepare(
            identity.clone(),
            custom.reference,
            installation.root.clone(),
            custom.adapter,
        )
        .await?;
        let command = custom_command_spec(&self.uv_bin, &self.data_plane_url, &installation)?;
        self.prepare_extension(installation);
        Ok(command)
    }

    /// Remove prepared associations and stop the node; stop failures retain the process for retry.
    pub async fn remove(&mut self, identity: &ApplyIdentity) -> Result<(), String> {
        let key = format!("{}/{}", identity.target.namespace, identity.target.name);
        if self
            .prepared_extensions
            .get(&key)
            .is_some_and(|installation| {
                installation.identity.target.uid == identity.target.uid
                    && installation.identity.generation <= identity.generation
            })
        {
            self.prepared_extensions.remove(&key);
        }
        let Some(node) = self.nodes.get_mut(&key) else {
            return Ok(());
        };
        // A stale remove MUST NOT stop a replacement resource.
        if node.identity.target.uid != identity.target.uid {
            return Ok(());
        }
        if node.identity.generation > identity.generation {
            return Err("stale cleanup generation".into());
        }
        if let Some(child) = node.child.as_mut() {
            child
                .stop_in_place(self.shutdown_grace)
                .await
                .map_err(|error| format!("failed to stop node: {error}"))?;
        }
        self.nodes.remove(&key);
        Ok(())
    }

    pub fn reap_exited(&mut self) -> Vec<NodeEvent> {
        let mut events = Vec::new();
        for node in self.nodes.values_mut() {
            let result = node
                .child
                .as_mut()
                .and_then(|child| match child.child.try_wait() {
                    Ok(Some(status)) => Some(Ok(status)),
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                });
            if let Some(result) = result {
                node.child = None;
                node.status = NodeRuntimeStatus::Failed;
                node.message = Some(match result {
                    Ok(status) => format!("node process exited unexpectedly: {status}"),
                    Err(error) => format!("failed to reap node process: {error}"),
                });
                events.push(node.event());
            }
        }
        events
    }

    /// Stop child processes and discard prepared installation associations.
    pub async fn shutdown_all(&mut self) -> Vec<NodeEvent> {
        self.prepared_extensions.clear();
        for node in self.nodes.values_mut() {
            if let Some(child) = node.child.as_mut() {
                let _ = child.terminate();
            }
        }
        let deadline = tokio::time::Instant::now() + self.shutdown_grace;
        let mut events = Vec::new();
        for node in self.nodes.values_mut() {
            if let Some(child) = node.child.take() {
                match child
                    .stop(deadline.saturating_duration_since(tokio::time::Instant::now()))
                    .await
                {
                    Ok(_) => {
                        node.status = NodeRuntimeStatus::Stopped;
                        node.message = None;
                    }
                    Err(error) => {
                        node.status = NodeRuntimeStatus::Failed;
                        node.message = Some(format!("failed to stop node: {error}"));
                    }
                }
                events.push(node.event());
            }
        }
        events
    }

    pub fn snapshots(&self) -> Vec<NodeSnapshot> {
        let mut snapshots = self
            .nodes
            .values()
            .map(|node| {
                let command = node.command.as_ref();
                NodeSnapshot {
                    name: node.spec.display_name.clone(),
                    node_type: match &node.spec.settings {
                        NodeSettings::AudioSource(_) => "Audio source",
                        NodeSettings::Transcriber(_) => "Transcriber",
                        NodeSettings::Translator(_) => "Translator",
                        NodeSettings::Display(_) => "Display",
                        NodeSettings::Custom(_) => "Custom",
                    },
                    status: node.status.as_str(),
                    parameters: command
                        .map(|command| {
                            command
                                .args
                                .iter()
                                .map(|value| value.to_string_lossy().into_owned())
                                .collect()
                        })
                        .unwrap_or_default(),
                    message: node.message.clone(),
                }
            })
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.name.cmp(&right.name));
        snapshots
    }
}
impl ManagedNode {
    fn event(&self) -> NodeEvent {
        NodeEvent {
            identity: self.identity.clone(),
            status: self.status,
            message: self.message.clone(),
        }
    }
}

pub fn data_plane_url_from_registration(registration: &Url) -> Result<Url, String> {
    let mut result = registration.clone();
    validate_data_plane_url(&result)?;
    let path = result.path().trim_end_matches('/');
    let prefix = path
        .strip_suffix("/register")
        .ok_or_else(|| "registration URL path must end in /register".to_owned())?;
    result.set_path(&format!("{prefix}/"));
    result.set_query(None);
    result.set_fragment(None);
    Ok(result)
}

fn validate_data_plane_url(url: &Url) -> Result<(), String> {
    if !matches!(url.scheme(), "ws" | "wss") {
        return Err("data-plane URL must use ws or wss".to_owned());
    }
    if url.host().is_none() {
        return Err("data-plane URL must include a host".to_owned());
    }
    Ok(())
}

/// Build ordered arguments without shell evaluation. Structured and null values require explicit conversion.
fn custom_command_spec(
    uv_bin: &Path,
    data_plane_url: &Url,
    installation: &PreparedExtension,
) -> io::Result<CommandSpec> {
    let adapter = &installation.adapter;
    let (executable, mut args) = match &installation.command {
        ExecutionCommand::Executable(path) => (path.clone(), Vec::new()),
        ExecutionCommand::Python(path) => (
            uv_bin.to_owned(),
            vec![
                "uv".into(),
                "run".into(),
                "python".into(),
                path.as_os_str().to_owned(),
            ],
        ),
        ExecutionCommand::PythonModule(module) => (
            uv_bin.to_owned(),
            vec![
                "uv".into(),
                "run".into(),
                "python".into(),
                "-m".into(),
                module.into(),
            ],
        ),
    };
    for binding in &adapter.arguments {
        let runtime = serde_json::Value::String(data_plane_url.as_str().to_owned());
        let composed;
        let value = match &binding.value {
            ArgumentValue::Source(ArgumentSource::Runtime(RuntimeValue::WebsocketUrl)) => &runtime,
            ArgumentValue::Source(ArgumentSource::Setting(key)) => adapter
                .settings
                .get(key)
                .ok_or_else(|| io::Error::other(format!("missing argument setting: {key}")))?,
            ArgumentValue::Url(url) => {
                let key = url.path.key();
                let path = adapter
                    .settings
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        io::Error::other(format!("URL path setting MUST be a string: {key}"))
                    })?;
                composed =
                    serde_json::Value::String(websocket_url_with_path(data_plane_url, path)?);
                &composed
            }
        };
        match binding.mode {
            ArgumentMode::Flag => {
                let enabled = value
                    .as_bool()
                    .ok_or_else(|| io::Error::other("flag argument MUST be boolean"))?;
                let flag = binding
                    .flag
                    .as_ref()
                    .ok_or_else(|| io::Error::other("flag mode MUST have a flag"))?;
                if enabled {
                    args.push(flag.into());
                }
            }
            ArgumentMode::Value => {
                let value = match value {
                    serde_json::Value::String(value) => value.clone(),
                    serde_json::Value::Bool(value) => value.to_string(),
                    serde_json::Value::Number(value) => value.to_string(),
                    _ => {
                        return Err(io::Error::other(
                            "argument value MUST be a string, number, or boolean; null and structured values require explicit conversion",
                        ));
                    }
                };
                if let Some(flag) = &binding.flag {
                    args.push(flag.into());
                }
                args.push(value.into());
            }
        }
    }
    Ok(CommandSpec {
        executable,
        args,
        current_dir: installation.working_directory.clone(),
    })
}

/// Append an endpoint path while retaining the configured origin, query, and path prefix.
fn websocket_url_with_path(base: &Url, path: &str) -> io::Result<String> {
    if path.is_empty() {
        return Ok(base.as_str().to_owned());
    }
    if path.starts_with("//") || path.contains('\\') || Url::parse(path).is_ok() {
        return Err(io::Error::other(
            "URL endpoint MUST be a path, not an origin",
        ));
    }
    let prefix = format!("{}/", base.path().trim_end_matches('/'));
    let mut url = base.clone();
    url.set_path(&format!("{prefix}{}", path.trim_start_matches('/')));
    if !url.path().starts_with(&prefix) {
        return Err(io::Error::other(
            "URL endpoint MUST retain the configured path prefix",
        ));
    }
    Ok(url.into())
}

/// Build a built-in command, deriving its client-specific address from the base URL.
pub fn command_spec(
    uv_bin: &Path,
    project_dir: &Path,
    data_plane_url: &Url,
    update: &SuntitleNodeSpec,
) -> io::Result<CommandSpec> {
    let mut args: Vec<OsString> = vec!["uv".into(), "run".into(), "python".into()];
    args.extend(match &update.settings {
        NodeSettings::AudioSource(settings) => settings.command_args(data_plane_url.as_str()),
        NodeSettings::Transcriber(settings) => settings.command_args(
            data_plane_url
                .join("transcribe")
                .map_err(io::Error::other)?
                .as_str(),
        ),
        NodeSettings::Translator(settings) => settings
            .command_args(&data_plane_url[url::Position::BeforeHost..url::Position::AfterPort]),
        NodeSettings::Display(settings) => settings
            .command_args(&data_plane_url[url::Position::BeforeHost..url::Position::AfterPort]),
        NodeSettings::Custom(_) => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "custom nodes require prepared execution context",
            ));
        }
    });
    Ok(CommandSpec {
        executable: uv_bin.to_path_buf(),
        args,
        current_dir: project_dir.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn test_spec(mut json: serde_json::Value) -> SuntitleNodeSpec {
        let kind = json["type"].as_str().unwrap().to_owned();
        json["settings"] = serde_json::json!({kind: json["settings"].clone()});
        json["hostRef"] = serde_json::json!({"name":"host"});
        serde_json::from_value(json).unwrap()
    }

    fn test_identity() -> ApplyIdentity {
        ApplyIdentity {
            target: suntitle_core::kubernetes::resources::ResourceIdentity {
                namespace: "suntitle".into(),
                name: "screen".into(),
                uid: Uuid::new_v4().to_string(),
            },
            host_uid: "host-uid".into(),
            session_id: "session".into(),
            generation: 1,
            restart_token: None,
            operation_id: "apply-1".into(),
        }
    }

    fn spec(json: serde_json::Value) -> CommandSpec {
        let update = test_spec(json);
        command_spec(
            Path::new("fake uv"),
            Path::new("/project with spaces"),
            &Url::parse("wss://example.test:9443/prefix/").unwrap(),
            &update,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn custom_apply_without_execution_context_fails_without_spawning() {
        let mut manager = NodeProcessManager::new(
            std::env::current_dir().unwrap(),
            PathBuf::from("/does-not-exist"),
            Url::parse("ws://localhost/").unwrap(),
            Duration::from_millis(10),
        )
        .unwrap();
        let custom: SuntitleNodeSpec = serde_json::from_value(serde_json::json!({
            "displayName":"Custom", "hostRef":{"name":"host"}, "extensionRef":{"name":"plugin"},
            "settings":{"custom":{}}
        }))
        .unwrap();
        let events = manager.apply(test_identity(), custom, None).await;
        assert_eq!(events[0].status, NodeRuntimeStatus::Failed);
        assert!(
            events[0]
                .message
                .as_deref()
                .unwrap()
                .contains("custom nodes require prepared execution context")
        );
        let snapshots = manager.snapshots();
        assert_eq!(snapshots[0].node_type, "Custom");
        assert!(snapshots[0].parameters.is_empty());
    }

    /// Native launch, argument updates, restart, and disable reuse the prepared payload.
    #[cfg(unix)]
    #[tokio::test]
    async fn custom_native_execution_reuses_installation_and_supervises_restarts() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("run");
        std::fs::write(&binary, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut manager = NodeProcessManager::new(
            root.path().into(),
            "/missing-uv".into(),
            Url::parse("ws://localhost/").unwrap(),
            Duration::from_millis(100),
        )
        .unwrap();
        let mut identity = test_identity();
        let mut spec: SuntitleNodeSpec = serde_json::from_value(serde_json::json!({"displayName":"Custom","hostRef":{"name":"host"},"extensionRef":{"name":"plugin"},"settings":{"custom":{}}})).unwrap();
        let adapter = serde_json::from_value(serde_json::json!({"inputType":"text","outputType":"text","execution":{"command":"./run"},"settings":{"value":"a b;$(ignored)"},"arguments":[{"flag":"--value","source":"settings.value"}]})).unwrap();
        let mut custom = CustomExecution {
            reference: "registry/extensions@sha256:test".into(),
            adapter,
        };
        let installation = PreparedExtension::prepare(
            identity.clone(),
            custom.reference.clone(),
            root.path().into(),
            custom.adapter.clone(),
        )
        .await
        .unwrap();
        manager.prepare_extension(installation);
        assert_eq!(
            manager
                .apply(identity.clone(), spec.clone(), Some(custom.clone()))
                .await[0]
                .status,
            NodeRuntimeStatus::Running
        );
        let key = format!("{}/{}", identity.target.namespace, identity.target.name);
        let first = manager.nodes[&key].child.as_ref().unwrap().child.id();
        assert_eq!(
            manager.snapshots()[0].parameters,
            ["--value", "a b;$(ignored)"]
        );
        identity.operation_id = "duplicate-configuration".into();
        manager
            .apply(identity.clone(), spec.clone(), Some(custom.clone()))
            .await;
        assert_eq!(
            manager.nodes[&key].child.as_ref().unwrap().child.id(),
            first
        );
        identity.generation += 1;
        identity.operation_id = "new-arguments".into();
        custom
            .adapter
            .settings
            .insert("value".into(), serde_json::json!("changed"));
        assert_eq!(
            manager
                .apply(identity.clone(), spec.clone(), Some(custom.clone()))
                .await[0]
                .status,
            NodeRuntimeStatus::Running
        );
        assert_ne!(
            manager.nodes[&key].child.as_ref().unwrap().child.id(),
            first
        );
        assert_eq!(
            manager.prepared_extension(&identity).unwrap().root,
            std::fs::canonicalize(root.path()).unwrap()
        );
        identity.operation_id = "disabled".into();
        identity.generation += 1;
        spec.enabled = false;
        assert_eq!(
            manager.apply(identity.clone(), spec, Some(custom)).await[0].status,
            NodeRuntimeStatus::Stopped
        );
        assert!(manager.nodes[&key].child.is_none());
        manager.remove(&identity).await.unwrap();
    }

    /// Python variants use embedded UV and every media type receives the unchanged base URL.
    #[tokio::test]
    async fn custom_command_modes_validate_values_and_preserve_base_url() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("plugin.py"), "pass").unwrap();
        let data_plane_url = Url::parse("wss://example.test:9443/prefix/").unwrap();
        for output in ["image", "text", "audio"] {
            for (runtime, entry) in [("python", "./plugin.py"), ("pythonModule", "plugin.main")] {
                let adapter = serde_json::from_value(serde_json::json!({"inputType":"audio","outputType":output,"execution":{"runtime":runtime,"command":entry},"settings":{"enabled":true,"zero":0,"empty":""},"arguments":[{"flag":"--server","source":"runtime.websocketUrl"},{"flag":"--on","source":"settings.enabled","mode":"flag"},{"source":"settings.zero"},{"source":"settings.empty"}]})).unwrap();
                let mut installed = PreparedExtension::prepare(
                    test_identity(),
                    "artifact".into(),
                    root.path().into(),
                    adapter,
                )
                .await
                .unwrap();
                let command = custom_command_spec(
                    Path::new("/embedded-diamant"),
                    &data_plane_url,
                    &installed,
                )
                .unwrap();
                assert_eq!(command.executable, Path::new("/embedded-diamant"));
                assert_eq!(
                    command.current_dir,
                    std::fs::canonicalize(root.path()).unwrap()
                );
                assert_eq!(
                    &command.args[..3],
                    &[OsString::from("uv"), "run".into(), "python".into()]
                );
                if runtime == "pythonModule" {
                    assert_eq!(
                        &command.args[3..5],
                        &[OsString::from("-m"), "plugin.main".into()]
                    );
                }
                let tail = &command.args[command.args.len() - 5..];
                assert_eq!(
                    tail,
                    &[
                        OsString::from("--server"),
                        "wss://example.test:9443/prefix/".into(),
                        "--on".into(),
                        "0".into(),
                        "".into()
                    ]
                );
                for invalid in [
                    serde_json::Value::Null,
                    serde_json::json!([]),
                    serde_json::json!({}),
                ] {
                    installed.adapter.settings.insert("zero".into(), invalid);
                    assert!(
                        custom_command_spec(
                            Path::new("/embedded-diamant"),
                            &data_plane_url,
                            &installed
                        )
                        .is_err()
                    );
                }
                installed
                    .adapter
                    .settings
                    .insert("zero".into(), serde_json::json!(0));
                installed
                    .adapter
                    .settings
                    .insert("enabled".into(), serde_json::json!(false));
                assert!(
                    !custom_command_spec(
                        Path::new("/embedded-diamant"),
                        &data_plane_url,
                        &installed
                    )
                    .unwrap()
                    .args
                    .contains(&"--on".into())
                );
            }
        }
    }

    /// URL bindings MUST inherit the host URL while the user supplies only an endpoint path.
    #[tokio::test]
    async fn url_arguments_compose_runtime_base_and_setting_path() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("plugin.py"), "pass").unwrap();
        for (base, path, expected) in [
            (
                "ws://127.0.0.1:3030/",
                "transcribe",
                "ws://127.0.0.1:3030/transcribe",
            ),
            (
                "wss://example.test:9443/prefix/",
                "translate",
                "wss://example.test:9443/prefix/translate",
            ),
            (
                "wss://example.test:9443/prefix",
                "/translate",
                "wss://example.test:9443/prefix/translate",
            ),
            (
                "wss://example.test:9443/prefix/?token=x",
                "transcribe",
                "wss://example.test:9443/prefix/transcribe?token=x",
            ),
            (
                "wss://example.test:9443/prefix/",
                "",
                "wss://example.test:9443/prefix/",
            ),
        ] {
            let adapter = serde_json::from_value(serde_json::json!({
                "execution":{"runtime":"python","command":"./plugin.py"},
                "settings":{"endpoint":path,"message":"$(runtime.websocketUrl)"},
                "arguments":[{"flag":"--server","url":{"base":"runtime.websocketUrl","path":"settings.endpoint"}},
                    {"source":"settings.message"}]
            })).unwrap();
            let mut installed = PreparedExtension::prepare(
                test_identity(),
                "artifact".into(),
                root.path().into(),
                adapter,
            )
            .await
            .unwrap();
            let command = custom_command_spec(
                Path::new("/embedded-diamant"),
                &Url::parse(base).unwrap(),
                &installed,
            )
            .unwrap();
            assert_eq!(
                &command.args[command.args.len() - 3..],
                &[
                    OsString::from("--server"),
                    expected.into(),
                    "$(runtime.websocketUrl)".into()
                ]
            );
            assert_eq!(installed.adapter.settings["endpoint"], path);
            for invalid in [
                serde_json::Value::Null,
                serde_json::json!(42),
                serde_json::json!("wss://other.test/"),
                serde_json::json!("//other.test/"),
                serde_json::json!("../escape"),
            ] {
                installed
                    .adapter
                    .settings
                    .insert("endpoint".into(), invalid);
                let prefixed = Url::parse("wss://example.test/prefix/").unwrap();
                assert!(
                    custom_command_spec(Path::new("/embedded-diamant"), &prefixed, &installed)
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn derives_base_url_without_losing_security_port_or_prefix() {
        let registration = Url::parse("wss://example.test:9443/prefix/register?token=x").unwrap();
        assert_eq!(
            data_plane_url_from_registration(&registration)
                .unwrap()
                .as_str(),
            "wss://example.test:9443/prefix/"
        );
    }

    #[test]
    fn rejects_non_websocket_data_plane_urls() {
        assert!(validate_data_plane_url(&Url::parse("https://example.test/").unwrap()).is_err());
    }

    #[test]
    fn audio_arguments_are_separate_and_not_shell_interpreted() {
        let result = spec(serde_json::json!({
            "type":"audio_source", "displayName":"mic", "enabled":true, "description":null,
            "settings":{"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le","deviceName":"USB; touch /tmp/nope"},
            "inputNodeId":null
        }));
        assert_eq!(result.executable, PathBuf::from("fake uv"));
        assert_eq!(result.current_dir, PathBuf::from("/project with spaces"));
        assert_eq!(
            result.args,
            vec![
                "uv",
                "run",
                "python",
                "suntitle-listener.py",
                "wss://example.test:9443/prefix/",
                "--samplerate=16000",
                "--channels=1",
                "--device=USB; touch /tmp/nope",
                "--format=raw"
            ]
        );
    }

    #[test]
    fn transcriber_arguments_include_every_runtime_setting() {
        let result = spec(serde_json::json!({
            "type":"transcriber", "displayName":"asr", "enabled":true, "description":null,
            "settings":{"model":"large-v3","language":"en","task":"translate","beamSize":8,"vadEnabled":true},
            "inputNodeId":null
        }));
        assert_eq!(
            result.args,
            vec![
                "uv",
                "run",
                "python",
                "whisper_online_client.py",
                "--uri=wss://example.test:9443/prefix/transcribe",
                "--model=large-v3",
                "--language=en",
                "--task=translate",
                "--vad",
                "--beam-size=8"
            ]
        );
    }

    #[test]
    fn translator_and_display_arguments_are_complete() {
        let translator = spec(serde_json::json!({
            "type":"translator", "displayName":"translate", "enabled":true, "description":null,
            "settings":{"sourceLanguage":"uk","targetLanguage":"cs","provider":"huggingface","model":"custom model","preserveFormatting":false},
            "inputNodeId":null
        }));
        assert_eq!(
            translator.args,
            vec![
                "uv",
                "run",
                "python",
                "suntitle-translator.py",
                "--host=example.test:9443",
                "--source-language=uk",
                "--target-language=cs",
                "--provider=huggingface",
                "--model=custom model",
                "--no-preserve-formatting"
            ]
        );

        let display = spec(serde_json::json!({
            "type":"display", "displayName":"screen", "enabled":true, "description":null,
            "settings":{"maxLines":3,"fontScale":1.5,"showOriginal":true,"theme":"transparent"},
            "inputNodeId":null
        }));
        assert_eq!(
            display.args,
            vec![
                "uv",
                "run",
                "python",
                "suntitle.py",
                "--host=example.test:9443",
                "--max-lines=3",
                "--font-scale=1.5",
                "--show-original",
                "--theme=transparent"
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn duplicate_apply_and_remove_preserve_identity_and_reap_child() {
        let directory = std::env::temp_dir().join(format!("diamant-nodes-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("fake-uv");
        std::fs::write(
            &executable,
            "#!/bin/sh\necho $$ >> pids.txt\ntrap 'exit 0' TERM\nwhile :; do sleep 1; done\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let mut manager = NodeProcessManager::new(
            directory.clone(),
            executable,
            Url::parse("ws://localhost:3030/").unwrap(),
            Duration::from_secs(2),
        )
        .unwrap();
        let update = test_spec(serde_json::json!({
            "type":"display", "displayName":"screen", "enabled":true, "description":null,
            "settings":{"maxLines":3,"fontScale":1.0,"showOriginal":false,"theme":"dark"}, "inputNodeId":null
        }));
        let identity = test_identity();

        let started = manager.apply(identity.clone(), update.clone(), None).await;
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].status, NodeRuntimeStatus::Running, "{started:?}");
        let snapshots = manager.snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].name, "screen");
        assert_eq!(snapshots[0].node_type, "Display");
        assert_eq!(snapshots[0].status, "Running");
        assert_eq!(snapshots[0].parameters[3], "suntitle.py");
        let pid_file = directory.join("pids.txt");
        for _ in 0..40 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            manager
                .apply(identity.clone(), update.clone(), None)
                .await
                .len(),
            1
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            std::fs::read_to_string(&pid_file).unwrap().lines().count(),
            1
        );

        let key = format!("{}/{}", identity.target.namespace, identity.target.name);
        let process_id = manager.nodes[&key].child.as_ref().unwrap().child.id();
        let mut pipeline_spec = update.clone();
        pipeline_spec.input_ref = Some(suntitle_core::kubernetes::resources::ResourceRef {
            name: "pipeline-upstream".into(),
        });
        let mut pipeline_identity = identity.clone();
        pipeline_identity.generation += 1;
        pipeline_identity.operation_id = "pipeline-change".into();
        assert_eq!(
            manager.apply(pipeline_identity, pipeline_spec, None).await[0].status,
            NodeRuntimeStatus::Running
        );
        assert_eq!(
            manager.nodes[&key].child.as_ref().unwrap().child.id(),
            process_id
        );

        let mut restart = identity;
        restart.generation += 2;
        restart.restart_token = Some("latest-coalesced-token".into());
        restart.operation_id = "restart".into();
        let mut restarted_spec = update;
        restarted_spec.restart_token = restart.restart_token.clone();
        assert_eq!(
            manager
                .apply(restart.clone(), restarted_spec.clone(), None)
                .await[0]
                .status,
            NodeRuntimeStatus::Running
        );
        assert_eq!(
            manager.apply(restart.clone(), restarted_spec, None).await[0].status,
            NodeRuntimeStatus::Running
        );
        for _ in 0..40 {
            if std::fs::read_to_string(&pid_file).unwrap().lines().count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&pid_file).unwrap().lines().count(),
            2
        );

        let mut stale = restart.clone();
        stale.target.uid = "old-incarnation".into();
        manager.remove(&stale).await.unwrap();
        assert_eq!(manager.nodes.len(), 1);
        let pid = manager
            .nodes
            .values()
            .next()
            .unwrap()
            .child
            .as_ref()
            .unwrap()
            .child
            .id()
            .unwrap();
        manager.remove(&restart).await.unwrap();
        manager.remove(&restart).await.unwrap();
        assert!(manager.nodes.is_empty());
        assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_err());
        assert!(manager.shutdown_all().await.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

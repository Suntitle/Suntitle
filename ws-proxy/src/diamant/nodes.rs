//! Child-process ownership for nodes assigned to a Diamant host.
//!
//! On Unix, every node is launched in its own process group. Shutdown sends
//! SIGTERM to the complete group, waits for the configured grace period, and
//! then uses SIGKILL as a bounded fallback.

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

use crate::kubernetes::resources::{NodeSettings, SuntitleNodeSpec};
use crate::registration::ApplyIdentity;

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
}

#[derive(Debug)]
pub struct NodeProcessManager {
    project_dir: PathBuf,
    uv_bin: PathBuf,
    endpoints: NodeEndpoints,
    shutdown_grace: Duration,
    nodes: HashMap<String, ManagedNode>,
}
impl NodeProcessManager {

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
        Ok(Self {
            project_dir,
            uv_bin,
            endpoints: NodeEndpoints::new(&data_plane_url)?,
            shutdown_grace,
            nodes: HashMap::new(),
        })
    }

    pub async fn apply(
        &mut self,
        identity: ApplyIdentity,
        spec: SuntitleNodeSpec,
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
            if node.identity.target.uid == identity.target.uid
                && node.status != NodeRuntimeStatus::Failed
                && runtime_spec(&node.spec) == runtime_spec(&spec)
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
        let (child, status, message) = if spec.enabled {
            match ChildProcess::spawn(&command_spec(
                &self.uv_bin,
                &self.project_dir,
                &self.endpoints,
                &spec,
            )) {
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
        };
        let event = node.event();
        self.nodes.insert(key, node);
        vec![event]
    }

    /// Stop failures retain the managed process for later cleanup.
    pub async fn remove(&mut self, identity: &ApplyIdentity) -> Result<(), String> {
        let key = format!("{}/{}", identity.target.namespace, identity.target.name);
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

    pub async fn shutdown_all(&mut self) -> Vec<NodeEvent> {
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
                let command = command_spec(
                    &self.uv_bin,
                    &self.project_dir,
                    &self.endpoints,
                    &node.spec,
                );
                NodeSnapshot {
                    name: node.spec.name.clone(),
                    node_type: match &node.spec.settings {
                        NodeSettings::AudioSource(_) => "Audio source",
                        NodeSettings::Transcriber(_) => "Transcriber",
                        NodeSettings::Translator(_) => "Translator",
                        NodeSettings::Display(_) => "Display",
                    },
                    status: node.status.as_str(),
                    parameters: command
                        .args
                        .iter()
                        .map(|value| value.to_string_lossy().into_owned())
                        .collect(),
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

fn runtime_spec(spec: &SuntitleNodeSpec) -> serde_json::Value {
    serde_json::json!([
        spec.enabled,
        spec.settings,
        spec.input_ref,
        spec.restart_token
    ])
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

fn endpoint(base: &Url, suffix: &str) -> Result<String, String> {
    base.join(suffix)
        .map(|url| url.to_string())
        .map_err(|error| format!("invalid data-plane endpoint: {error}"))
}

fn authority(base: &Url) -> Result<String, String> {
    let host = base
        .host_str()
        .ok_or_else(|| "data-plane URL has no host".to_owned())?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Ok(match base.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

// TODO: NodeEndpoints must be moved on API level and imported here.
/// Data-plane addresses prepared once from validated host configuration.
#[derive(Debug)]
pub struct NodeEndpoints {
    audio: String,
    transcriber: String,
    authority: String,
}

impl NodeEndpoints {

    pub fn new(base: &Url) -> Result<Self, String> {
        validate_data_plane_url(base)?;
        Ok(Self {
            audio: endpoint(base, "")?,
            transcriber: endpoint(base, "transcribe")?,
            authority: authority(base)?,
        })
    }
}

/// Builds arguments for settings already validated at API submission.
pub fn command_spec(
    uv_bin: &Path,
    project_dir: &Path,
    endpoints: &NodeEndpoints,
    update: &SuntitleNodeSpec,
) -> CommandSpec {
    let mut args: Vec<OsString> = vec!["uv".into(), "run".into(), "python".into()];
    args.extend(match &update.settings {
        NodeSettings::AudioSource(settings) => settings.command_args(&endpoints.audio),
        NodeSettings::Transcriber(settings) => settings.command_args(&endpoints.transcriber),
        NodeSettings::Translator(settings) => settings.command_args(&endpoints.authority),
        NodeSettings::Display(settings) => settings.command_args(&endpoints.authority),
    });
    CommandSpec {
        executable: uv_bin.to_path_buf(),
        args,
        current_dir: project_dir.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn test_spec(mut json: serde_json::Value) -> SuntitleNodeSpec {
        let kind = json["type"].as_str().unwrap().to_owned();
        json["settings"] = serde_json::json!({kind: json["settings"].clone()});
        json["hostRef"] = serde_json::json!({"name":"host"});
        serde_json::from_value(json).unwrap()
    }

    fn test_identity() -> ApplyIdentity {
        ApplyIdentity {
            target: crate::kubernetes::resources::ResourceIdentity {
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
            &NodeEndpoints::new(&Url::parse("wss://example.test:9443/prefix/").unwrap()).unwrap(),
            &update,
        )
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
            "type":"audio_source", "name":"mic", "enabled":true, "description":null,
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
            "type":"transcriber", "name":"asr", "enabled":true, "description":null,
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
            "type":"translator", "name":"translate", "enabled":true, "description":null,
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
            "type":"display", "name":"screen", "enabled":true, "description":null,
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
            "type":"display", "name":"screen", "enabled":true, "description":null,
            "settings":{"maxLines":3,"fontScale":1.0,"showOriginal":false,"theme":"dark"}, "inputNodeId":null
        }));
        let identity = test_identity();

        let started = manager.apply(identity.clone(), update.clone()).await;
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
            manager.apply(identity.clone(), update.clone()).await.len(),
            1
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            std::fs::read_to_string(&pid_file).unwrap().lines().count(),
            1
        );

        let mut restart = identity;
        restart.generation += 2;
        restart.restart_token = Some("latest-coalesced-token".into());
        restart.operation_id = "restart".into();
        let mut restarted_spec = update;
        restarted_spec.restart_token = restart.restart_token.clone();
        assert_eq!(
            manager.apply(restart.clone(), restarted_spec.clone()).await[0].status,
            NodeRuntimeStatus::Running
        );
        assert_eq!(
            manager.apply(restart.clone(), restarted_spec).await[0].status,
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

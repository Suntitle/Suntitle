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
use uuid::Uuid;

use crate::api::models::{
    AudioEncoding, DisplayTheme, LanguageTag, NodeType, NodeUpdate, TranscriberTask,
};
use crate::registration::NodeRuntimeStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeEvent {
    pub node_id: Uuid,
    pub revision: u64,
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
    revision: u64,
    update: NodeUpdate,
    child: Option<ChildProcess>,
}

#[derive(Debug)]
pub struct NodeProcessManager {
    project_dir: PathBuf,
    uv_bin: PathBuf,
    data_plane_url: Url,
    shutdown_grace: Duration,
    nodes: HashMap<Uuid, ManagedNode>,
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
        validate_data_plane_url(&data_plane_url)?;
        Ok(Self {
            project_dir,
            uv_bin,
            data_plane_url,
            shutdown_grace,
            nodes: HashMap::new(),
        })
    }

    pub async fn apply(
        &mut self,
        node_id: Uuid,
        revision: u64,
        update: NodeUpdate,
    ) -> Vec<NodeEvent> {
        if self
            .nodes
            .get(&node_id)
            .is_some_and(|node| revision <= node.revision)
        {
            return Vec::new();
        }
        if let Some(node) = self.nodes.get_mut(&node_id)
            && node.update == update
        {
            node.revision = revision;
            return vec![event(
                node_id,
                revision,
                if node.child.is_some() {
                    NodeRuntimeStatus::Running
                } else {
                    NodeRuntimeStatus::Stopped
                },
                None,
            )];
        }

        let spec = match command_spec(
            &self.uv_bin,
            &self.project_dir,
            &self.data_plane_url,
            &update,
        ) {
            Ok(spec) => spec,
            Err(message) => {
                if let Some(old) = self.nodes.remove(&node_id)
                    && let Some(child) = old.child
                {
                    let _ = child.stop(self.shutdown_grace).await;
                }
                self.nodes.insert(
                    node_id,
                    ManagedNode {
                        revision,
                        update,
                        child: None,
                    },
                );
                return vec![event(
                    node_id,
                    revision,
                    NodeRuntimeStatus::Failed,
                    Some(message),
                )];
            }
        };

        if let Some(old) = self.nodes.remove(&node_id)
            && let Some(child) = old.child
        {
            let _ = child.stop(self.shutdown_grace).await;
        }

        if !update.enabled() {
            self.nodes.insert(
                node_id,
                ManagedNode {
                    revision,
                    update,
                    child: None,
                },
            );
            return vec![event(node_id, revision, NodeRuntimeStatus::Stopped, None)];
        }

        let mut events = vec![event(node_id, revision, NodeRuntimeStatus::Starting, None)];
        match ChildProcess::spawn(&spec) {
            Ok(child) => {
                self.nodes.insert(
                    node_id,
                    ManagedNode {
                        revision,
                        update,
                        child: Some(child),
                    },
                );
                events.push(event(node_id, revision, NodeRuntimeStatus::Running, None));
            }
            Err(error) => {
                self.nodes.insert(
                    node_id,
                    ManagedNode {
                        revision,
                        update,
                        child: None,
                    },
                );
                events.push(event(
                    node_id,
                    revision,
                    NodeRuntimeStatus::Failed,
                    Some(format!("failed to spawn node: {error}")),
                ));
            }
        }
        events
    }

    pub async fn restart(&mut self, node_id: Uuid, revision: u64) -> Vec<NodeEvent> {
        let Some(node) = self.nodes.remove(&node_id) else {
            return vec![event(
                node_id,
                revision,
                NodeRuntimeStatus::Failed,
                Some("cannot restart unknown node".to_owned()),
            )];
        };
        if revision <= node.revision {
            self.nodes.insert(node_id, node);
            return Vec::new();
        }
        let update = node.update;
        if let Some(child) = node.child {
            let _ = child.stop(self.shutdown_grace).await;
        }
        self.apply(node_id, revision, update).await
    }

    pub fn reap_exited(&mut self) -> Vec<NodeEvent> {
        let mut events = Vec::new();
        for (node_id, node) in &mut self.nodes {
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
                let message = match result {
                    Ok(status) => format!("node process exited unexpectedly: {status}"),
                    Err(error) => format!("failed to reap node process: {error}"),
                };
                events.push(event(
                    *node_id,
                    node.revision,
                    NodeRuntimeStatus::Failed,
                    Some(message),
                ));
            }
        }
        events
    }

    pub async fn shutdown_all(&mut self) -> Vec<NodeEvent> {
        let mut children = Vec::new();
        for (node_id, node) in &mut self.nodes {
            if let Some(child) = node.child.take() {
                children.push((*node_id, node.revision, child));
            }
        }
        for (_, _, child) in &mut children {
            let _ = child.terminate();
        }
        let mut events = Vec::with_capacity(children.len());
        let deadline = tokio::time::Instant::now() + self.shutdown_grace;
        for (node_id, revision, mut child) in children {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let result = match tokio::time::timeout(remaining, child.child.wait()).await {
                Ok(result) => result,
                Err(_) => {
                    let _ = child.force_kill();
                    child.child.wait().await
                }
            };
            events.push(match result {
                Ok(_) => event(node_id, revision, NodeRuntimeStatus::Stopped, None),
                Err(error) => event(
                    node_id,
                    revision,
                    NodeRuntimeStatus::Failed,
                    Some(format!("failed to stop node: {error}")),
                ),
            });
        }
        events
    }
}

fn event(
    node_id: Uuid,
    revision: u64,
    status: NodeRuntimeStatus,
    message: Option<String>,
) -> NodeEvent {
    NodeEvent {
        node_id,
        revision,
        status,
        message,
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

fn language<'a>(tag: &'a LanguageTag, field: &str) -> Result<&'a str, String> {
    let value = tag.0.as_str();
    if (value.len() == 2 || value.len() == 3) && value.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        Ok(value)
    } else {
        Err(format!(
            "{field} must be a lowercase ISO 639 language code; regional BCP 47 tags are not supported"
        ))
    }
}

pub fn command_spec(
    uv_bin: &Path,
    project_dir: &Path,
    base: &Url,
    update: &NodeUpdate,
) -> Result<CommandSpec, String> {
    validate_data_plane_url(base)?;
    let mut args: Vec<OsString> = vec!["run".into(), "python".into()];
    match update {
        NodeUpdate::AudioSource(node) => {
            if node.variant.node_type != NodeType::AudioSource
                || node.base.node_type != NodeType::AudioSource
            {
                return Err("node type does not match audio source payload".to_owned());
            }
            if node.variant.settings.encoding != AudioEncoding::PcmS16le {
                return Err("audio encoding is unsupported; only pcm_s16le is available".to_owned());
            }
            if ![8000, 16000, 22050, 44100, 48000].contains(&node.variant.settings.sample_rate_hz) {
                return Err(
                    "audio sample rate must be one of 8000, 16000, 22050, 44100, or 48000"
                        .to_owned(),
                );
            }
            if !(1..=2).contains(&node.variant.settings.channels) {
                return Err("audio channels must be 1 or 2".to_owned());
            }
            args.extend([
                "suntitle-listener.py".into(),
                endpoint(base, "")?.into(),
                "--samplerate".into(),
                node.variant.settings.sample_rate_hz.to_string().into(),
                "--channels".into(),
                node.variant.settings.channels.to_string().into(),
            ]);
            if let Some(device) = &node.variant.settings.device_name {
                args.extend(["--device".into(), device.into()]);
            }
            args.extend(["--format".into(), "raw".into()]);
        }
        NodeUpdate::Transcriber(node) => {
            if node.variant.node_type != NodeType::Transcriber
                || node.base.node_type != NodeType::Transcriber
            {
                return Err("node type does not match transcriber payload".to_owned());
            }
            let settings = &node.variant.settings;
            let language = language(&settings.language, "transcriber language")?;
            const MODELS: &[&str] = &[
                "tiny.en",
                "tiny",
                "base.en",
                "base",
                "small.en",
                "small",
                "medium.en",
                "medium",
                "large-v1",
                "large-v2",
                "large-v3",
                "large",
                "large-v3-turbo",
            ];
            if !MODELS.contains(&settings.model.as_str()) {
                return Err(format!("unsupported transcriber model: {}", settings.model));
            }
            if !(1..=20).contains(&settings.beam_size) {
                return Err("transcriber beam size must be between 1 and 20".to_owned());
            }
            args.extend([
                "whisper_online_client.py".into(),
                "--uri".into(),
                endpoint(base, "transcribe")?.into(),
                "--model".into(),
                settings.model.clone().into(),
                "--language".into(),
                language.into(),
                "--task".into(),
                match settings.task {
                    TranscriberTask::Transcribe => "transcribe",
                    TranscriberTask::Translate => "translate",
                }
                .into(),
            ]);
            if settings.vad_enabled {
                args.push("--vad".into());
            }
            args.extend(["--beam-size".into(), settings.beam_size.to_string().into()]);
        }
        NodeUpdate::Translator(node) => {
            if node.variant.node_type != NodeType::Translator
                || node.base.node_type != NodeType::Translator
            {
                return Err("node type does not match translator payload".to_owned());
            }
            let settings = &node.variant.settings;
            let source = language(&settings.source_language, "translator source language")?;
            let target = language(&settings.target_language, "translator target language")?;
            if settings.provider != "huggingface" {
                return Err("translator provider must be huggingface".to_owned());
            }
            args.extend([
                "suntitle-translator.py".into(),
                "--host".into(),
                authority(base)?.into(),
                "--source-language".into(),
                source.into(),
                "--target-language".into(),
                target.into(),
                "--provider".into(),
                settings.provider.clone().into(),
            ]);
            if let Some(model) = &settings.model {
                args.extend(["--model".into(), model.into()]);
            }
            if !settings.preserve_formatting {
                args.push("--no-preserve-formatting".into());
            }
        }
        NodeUpdate::Display(node) => {
            if node.variant.node_type != NodeType::Display
                || node.base.node_type != NodeType::Display
            {
                return Err("node type does not match display payload".to_owned());
            }
            let settings = &node.variant.settings;
            if !(1..=10).contains(&settings.max_lines) {
                return Err("display max lines must be between 1 and 10".to_owned());
            }
            if !(0.5..=4.0).contains(&settings.font_scale) {
                return Err("display font scale must be between 0.5 and 4.0".to_owned());
            }
            args.extend([
                "suntitle.py".into(),
                "--host".into(),
                authority(base)?.into(),
                "--max-lines".into(),
                settings.max_lines.to_string().into(),
                "--font-scale".into(),
                settings.font_scale.to_string().into(),
            ]);
            if settings.show_original {
                args.push("--show-original".into());
            }
            args.extend([
                "--theme".into(),
                match settings.theme {
                    DisplayTheme::Light => "light",
                    DisplayTheme::Dark => "dark",
                    DisplayTheme::Transparent => "transparent",
                }
                .into(),
            ]);
        }
    }
    Ok(CommandSpec {
        executable: uv_bin.to_path_buf(),
        args,
        current_dir: project_dir.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn spec(json: serde_json::Value) -> CommandSpec {
        let update: NodeUpdate = serde_json::from_value(json).unwrap();
        command_spec(
            Path::new("fake uv"),
            Path::new("/project with spaces"),
            &Url::parse("wss://example.test:9443/prefix/").unwrap(),
            &update,
        )
        .unwrap()
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
                "run",
                "python",
                "suntitle-listener.py",
                "wss://example.test:9443/prefix/",
                "--samplerate",
                "16000",
                "--channels",
                "1",
                "--device",
                "USB; touch /tmp/nope",
                "--format",
                "raw"
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
                "run",
                "python",
                "whisper_online_client.py",
                "--uri",
                "wss://example.test:9443/prefix/transcribe",
                "--model",
                "large-v3",
                "--language",
                "en",
                "--task",
                "translate",
                "--vad",
                "--beam-size",
                "8"
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
                "run",
                "python",
                "suntitle-translator.py",
                "--host",
                "example.test:9443",
                "--source-language",
                "uk",
                "--target-language",
                "cs",
                "--provider",
                "huggingface",
                "--model",
                "custom model",
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
                "run",
                "python",
                "suntitle.py",
                "--host",
                "example.test:9443",
                "--max-lines",
                "3",
                "--font-scale",
                "1.5",
                "--show-original",
                "--theme",
                "transparent"
            ]
        );
    }

    #[test]
    fn unsupported_audio_and_regional_language_fail_before_spawn() {
        let audio: NodeUpdate = serde_json::from_value(serde_json::json!({
            "type":"audio_source", "name":"mic", "enabled":true, "description":null,
            "settings":{"sampleRateHz":16000,"channels":1,"encoding":"opus","deviceName":null}, "inputNodeId":null
        })).unwrap();
        assert!(
            command_spec(
                Path::new("uv"),
                Path::new("."),
                &Url::parse("ws://localhost/").unwrap(),
                &audio
            )
            .is_err()
        );
        let transcriber: NodeUpdate = serde_json::from_value(serde_json::json!({
            "type":"transcriber", "name":"asr", "enabled":true, "description":null,
            "settings":{"model":"large-v3","language":"en-US","task":"transcribe","beamSize":5,"vadEnabled":false}, "inputNodeId":null
        })).unwrap();
        assert!(
            command_spec(
                Path::new("uv"),
                Path::new("."),
                &Url::parse("ws://localhost/").unwrap(),
                &transcriber
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn duplicate_apply_does_not_spawn_twice_and_shutdown_reaps_child() {
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
        let update: NodeUpdate = serde_json::from_value(serde_json::json!({
            "type":"display", "name":"screen", "enabled":true, "description":null,
            "settings":{"maxLines":3,"fontScale":1.0,"showOriginal":false,"theme":"dark"}, "inputNodeId":null
        })).unwrap();
        let node_id = Uuid::new_v4();

        let started = manager.apply(node_id, 1, update.clone()).await;
        assert_eq!(started.len(), 2);
        assert_eq!(started[1].status, NodeRuntimeStatus::Running, "{started:?}");
        let pid_file = directory.join("pids.txt");
        for _ in 0..40 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(manager.apply(node_id, 2, update).await.len(), 1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            std::fs::read_to_string(pid_file).unwrap().lines().count(),
            1
        );

        let events = manager.shutdown_all().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status, NodeRuntimeStatus::Stopped);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

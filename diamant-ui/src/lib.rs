use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::QString;
use serde::{Deserialize, Serialize};
use url::Url;

static DEFAULT_SETTINGS: OnceLock<GuiSettings> = OnceLock::new();
static AGENT_LAUNCHER: OnceLock<AgentLauncher> = OnceLock::new();

type AgentLauncher = Arc<dyn Fn(AgentConfig, Arc<AtomicBool>, Reporter) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub proxy_url: String,
    pub host_name: String,
    pub host_ip: Option<IpAddr>,
    pub health_interval_seconds: u64,
    pub project_dir: PathBuf,
    pub data_plane_url: Option<Url>,
    pub shutdown_grace_seconds: u64,
}

#[cxx_qt::bridge]
mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, proxy_url, cxx_name = "proxyUrl")]
        #[qproperty(QString, host_name, cxx_name = "hostName")]
        #[qproperty(QString, host_ip, cxx_name = "hostIp")]
        #[qproperty(i32, health_interval, cxx_name = "healthInterval")]
        #[qproperty(QString, project_dir, cxx_name = "projectDir")]
        #[qproperty(QString, data_plane_url, cxx_name = "dataPlaneUrl")]
        #[qproperty(i32, shutdown_grace, cxx_name = "shutdownGrace")]
        #[qproperty(QString, connection_state, cxx_name = "connectionState")]
        #[qproperty(QString, connection_message, cxx_name = "connectionMessage")]
        #[qproperty(QString, nodes_json, cxx_name = "nodesJson")]
        #[qproperty(bool, running)]
        type DiamantController = super::DiamantControllerRust;

        #[qinvokable]
        #[cxx_name = "toggleConnection"]
        fn toggle_connection(self: Pin<&mut Self>);
    }

    impl cxx_qt::Threading for DiamantController {}
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GuiSettings {
    proxy_url: String,
    host_name: String,
    host_ip: String,
    health_interval: i32,
    project_dir: String,
    data_plane_url: String,
    shutdown_grace: i32,
}

impl From<&AgentConfig> for GuiSettings {
    fn from(config: &AgentConfig) -> Self {
        Self {
            proxy_url: config.proxy_url.clone(),
            host_name: config.host_name.clone(),
            host_ip: config
                .host_ip
                .map(|value| value.to_string())
                .unwrap_or_default(),
            health_interval: config.health_interval_seconds as i32,
            project_dir: config.project_dir.to_string_lossy().into_owned(),
            data_plane_url: config
                .data_plane_url
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            shutdown_grace: config.shutdown_grace_seconds as i32,
        }
    }
}

pub struct DiamantControllerRust {
    proxy_url: QString,
    host_name: QString,
    host_ip: QString,
    health_interval: i32,
    project_dir: QString,
    data_plane_url: QString,
    shutdown_grace: i32,
    connection_state: QString,
    connection_message: QString,
    nodes_json: QString,
    running: bool,
    stop: Option<Arc<AtomicBool>>,
}

impl Default for DiamantControllerRust {
    fn default() -> Self {
        let defaults = DEFAULT_SETTINGS
            .get()
            .cloned()
            .expect("Diamant UI defaults must be initialized");
        let settings = load_settings().unwrap_or(defaults);
        Self {
            proxy_url: QString::from(&settings.proxy_url),
            host_name: QString::from(&settings.host_name),
            host_ip: QString::from(&settings.host_ip),
            health_interval: settings.health_interval,
            project_dir: QString::from(&settings.project_dir),
            data_plane_url: QString::from(&settings.data_plane_url),
            shutdown_grace: settings.shutdown_grace,
            connection_state: QString::from("disconnected"),
            connection_message: QString::default(),
            nodes_json: QString::from("[]"),
            running: false,
            stop: None,
        }
    }
}

impl Drop for DiamantControllerRust {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

impl qobject::DiamantController {
    fn toggle_connection(mut self: core::pin::Pin<&mut Self>) {
        if *self.running() {
            if let Some(stop) = &self.rust().stop {
                stop.store(true, Ordering::Relaxed);
            }
            self.as_mut()
                .set_connection_state(QString::from("disconnecting"));
            return;
        }

        let settings = GuiSettings {
            proxy_url: self.proxy_url().to_string().trim().to_owned(),
            host_name: self.host_name().to_string().trim().to_owned(),
            host_ip: self.host_ip().to_string().trim().to_owned(),
            health_interval: *self.health_interval(),
            project_dir: self.project_dir().to_string().trim().to_owned(),
            data_plane_url: self.data_plane_url().to_string().trim().to_owned(),
            shutdown_grace: *self.shutdown_grace(),
        };
        let config = match settings.to_config() {
            Ok(config) => config,
            Err(message) => {
                self.as_mut().set_connection_state(QString::from("error"));
                self.as_mut()
                    .set_connection_message(QString::from(&message));
                return;
            }
        };
        if let Err(error) = save_settings(&settings) {
            self.as_mut().set_connection_state(QString::from("error"));
            self.as_mut().set_connection_message(QString::from(&error));
            return;
        }

        let stop = Arc::new(AtomicBool::new(false));
        self.as_mut().rust_mut().stop = Some(Arc::clone(&stop));
        self.as_mut().set_running(true);
        self.as_mut()
            .set_connection_state(QString::from("connecting"));
        self.as_mut().set_connection_message(QString::default());
        self.as_mut().set_nodes_json(QString::from("[]"));
        let reporter = Reporter {
            thread: self.qt_thread(),
        };
        let launcher = Arc::clone(
            AGENT_LAUNCHER
                .get()
                .expect("Diamant agent launcher must be initialized"),
        );
        std::thread::spawn(move || launcher(config, stop, reporter));
    }
}

impl GuiSettings {
    fn to_config(&self) -> Result<AgentConfig, String> {
        if self.proxy_url.is_empty() || self.host_name.is_empty() || self.project_dir.is_empty() {
            return Err("Proxy URL, host name, and project directory are required.".into());
        }
        let host_ip = if self.host_ip.is_empty() {
            None
        } else {
            Some(
                self.host_ip
                    .parse::<IpAddr>()
                    .map_err(|error| format!("Invalid host IP: {error}"))?,
            )
        };
        let data_plane_url = if self.data_plane_url.is_empty() {
            None
        } else {
            Some(
                Url::parse(&self.data_plane_url)
                    .map_err(|error| format!("Invalid data-plane URL: {error}"))?,
            )
        };
        Ok(AgentConfig {
            proxy_url: self.proxy_url.clone(),
            host_name: self.host_name.clone(),
            host_ip,
            health_interval_seconds: self.health_interval.max(1) as u64,
            project_dir: PathBuf::from(&self.project_dir),
            data_plane_url,
            shutdown_grace_seconds: self.shutdown_grace.max(1) as u64,
        })
    }
}

#[derive(Clone)]
pub struct Reporter {
    thread: CxxQtThread<qobject::DiamantController>,
}

impl Reporter {
    pub fn connection(&self, state: &str, message: &str) {
        let state = QString::from(state);
        let message = QString::from(message);
        let _ = self.thread.queue(move |mut controller| {
            controller.as_mut().set_connection_state(state);
            controller.as_mut().set_connection_message(message);
        });
    }

    pub fn nodes<T: Serialize>(&self, nodes: &T) {
        let json = QString::from(&serde_json::to_string(nodes).unwrap_or_else(|_| "[]".to_owned()));
        let _ = self
            .thread
            .queue(move |mut controller| controller.as_mut().set_nodes_json(json));
    }

    pub fn failed(&self, message: &str) {
        let message = QString::from(message);
        let _ = self.thread.queue(move |mut controller| {
            controller.as_mut().set_running(false);
            controller
                .as_mut()
                .set_connection_state(QString::from("error"));
            controller.as_mut().set_connection_message(message);
            controller.as_mut().rust_mut().stop = None;
        });
    }

    pub fn finished(&self) {
        let _ = self.thread.queue(|mut controller| {
            controller.as_mut().set_running(false);
            controller
                .as_mut()
                .set_connection_state(QString::from("disconnected"));
            controller.as_mut().set_nodes_json(QString::from("[]"));
            controller.as_mut().rust_mut().stop = None;
        });
    }
}

fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("suntitle").join("diamant.json"))
}

fn load_settings() -> Option<GuiSettings> {
    serde_json::from_slice(&fs::read(settings_path()?).ok()?).ok()
}

fn save_settings(settings: &GuiSettings) -> Result<(), String> {
    let path =
        settings_path().ok_or_else(|| "Unable to locate the settings directory.".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid settings path.".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create settings directory: {error}"))?;
    let json = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("Failed to encode settings: {error}"))?;
    fs::write(path, json).map_err(|error| format!("Failed to save settings: {error}"))
}

pub fn run<F>(config: AgentConfig, launcher: F) -> ExitCode
where
    F: Fn(AgentConfig, Arc<AtomicBool>, Reporter) + Send + Sync + 'static,
{
    let _ = DEFAULT_SETTINGS.set(GuiSettings::from(&config));
    let _ = AGENT_LAUNCHER.set(Arc::new(launcher));
    unsafe {
        std::env::set_var("QT_QUICK_CONTROLS_STYLE", "Basic");
    }
    let mut app = cxx_qt_lib::QGuiApplication::new();
    let mut engine = cxx_qt_lib::QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&cxx_qt_lib::QUrl::from(
            "qrc:/qt/qml/io/suntitle/diamant/qml/diamant.qml",
        ));
    }
    let code = app.as_mut().map(|app| app.exec()).unwrap_or(1);
    ExitCode::from(code.clamp(0, u8::MAX as i32) as u8)
}

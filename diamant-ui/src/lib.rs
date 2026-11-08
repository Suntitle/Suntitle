//! Native Diamant desktop UI built directly into the Rust executable.
//!
//! The window starts disconnected, transitions through `connecting` and
//! `connected`, and terminates a run in `disconnected` or `error`. Disconnect is
//! deferred until the agent acknowledges the shared stop flag.

use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};

use eframe::egui::{
    self, Align, Color32, Frame, Grid, Layout, Margin, RichText, Shadow, Stroke, TextEdit,
    TextureHandle, Ui, Vec2,
};
use serde::{Deserialize, Serialize};
use url::Url;

static DEFAULT_SETTINGS: OnceLock<GuiSettings> = OnceLock::new();
static AGENT_LAUNCHER: OnceLock<AgentLauncher> = OnceLock::new();
type AgentLauncher = Arc<dyn Fn(AgentConfig, Arc<AtomicBool>, Reporter) + Send + Sync>;

const BACKGROUND: Color32 = Color32::from_rgb(250, 248, 255);
const CARD: Color32 = Color32::WHITE;
const BORDER: Color32 = Color32::from_rgb(229, 229, 229);
const FOREGROUND: Color32 = Color32::from_rgb(10, 10, 10);
const MUTED: Color32 = Color32::from_rgb(115, 115, 115);
const VIOLET: Color32 = Color32::from_rgb(110, 86, 207);
const VIOLET_TEXT: Color32 = Color32::from_rgb(101, 80, 185);
const GREEN_BACKGROUND: Color32 = Color32::from_rgb(230, 246, 235);
const GREEN_TEXT: Color32 = Color32::from_rgb(33, 131, 88);

/// Configuration passed from the desktop UI to the Diamant agent.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// WebSocket registration endpoint exposed by the proxy.
    pub proxy_url: String,
    /// Stable name used to identify this host.
    pub host_name: String,
    /// Optional host address override.
    pub host_ip: Option<IpAddr>,
    /// Delay between health reports in seconds.
    pub health_interval_seconds: u64,
    /// Optional explicit WebSocket data-plane endpoint.
    pub data_plane_url: Option<Url>,
    /// Grace period for child-process shutdown in seconds.
    pub shutdown_grace_seconds: u64,
}

/// User-editable values persisted between GUI launches.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct GuiSettings {
    /// Registration proxy URL.
    proxy_url: String,
    /// Read-only host identity.
    host_name: String,
    /// Optional reported host address.
    host_ip: String,
    /// Health-report interval in seconds.
    health_interval: u64,
    /// Optional explicit data-plane URL.
    data_plane_url: String,
    /// Child-process shutdown grace period in seconds.
    shutdown_grace: u64,
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
            health_interval: config.health_interval_seconds,
            data_plane_url: config
                .data_plane_url
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            shutdown_grace: config.shutdown_grace_seconds,
        }
    }
}

impl GuiSettings {
    /// Validates editable strings and produces the strongly typed agent configuration.
    fn to_config(&self) -> Result<AgentConfig, String> {
        if self.proxy_url.trim().is_empty() || self.host_name.trim().is_empty() {
            return Err("Proxy URL and host name are required.".into());
        }
        let host_ip = if self.host_ip.trim().is_empty() {
            None
        } else {
            Some(
                self.host_ip
                    .trim()
                    .parse::<IpAddr>()
                    .map_err(|error| format!("Invalid host IP: {error}"))?,
            )
        };
        let data_plane_url = if self.data_plane_url.trim().is_empty() {
            None
        } else {
            Some(
                Url::parse(self.data_plane_url.trim())
                    .map_err(|error| format!("Invalid data-plane URL: {error}"))?,
            )
        };
        Ok(AgentConfig {
            proxy_url: self.proxy_url.trim().to_owned(),
            host_name: self.host_name.trim().to_owned(),
            host_ip,
            health_interval_seconds: self.health_interval.max(1),
            data_plane_url,
            shutdown_grace_seconds: self.shutdown_grace.max(1),
        })
    }
}

/// Cross-thread state update emitted by the running agent.
enum UiEvent {
    /// Updates connection state and its diagnostic.
    Connection { state: String, message: String },
    /// Replaces the serialized running-node snapshot.
    Nodes(String),
    /// Terminates the run with a visible error.
    Failed(String),
    /// Terminates the run normally.
    Finished,
}

/// Cloneable reporting endpoint owned by the agent thread.
#[derive(Clone)]
pub struct Reporter {
    /// Channel used to update the UI without toolkit-specific objects.
    sender: Sender<UiEvent>,
    /// Rendering context used to request a repaint.
    context: egui::Context,
}

impl Reporter {
    /// Reports a non-terminal connection transition.
    pub fn connection(&self, state: &str, message: &str) {
        self.send(UiEvent::Connection {
            state: state.to_owned(),
            message: message.to_owned(),
        });
    }

    /// Replaces the current node snapshot.
    pub fn nodes<T: Serialize>(&self, nodes: &T) {
        self.send(UiEvent::Nodes(
            serde_json::to_string(nodes).unwrap_or_else(|_| "[]".to_owned()),
        ));
    }

    /// Reports an agent failure and ends the run.
    pub fn failed(&self, message: &str) {
        self.send(UiEvent::Failed(message.to_owned()));
    }

    /// Reports normal termination and returns to disconnected state.
    pub fn finished(&self) {
        self.send(UiEvent::Finished);
    }

    /// Sends one event and repaints when the receiver remains alive.
    fn send(&self, event: UiEvent) {
        if self.sender.send(event).is_ok() {
            self.context.request_repaint();
        }
    }
}

/// Node fields rendered by the running-nodes table.
#[derive(Default, Deserialize)]
struct NodeSnapshot {
    /// User-facing node name.
    #[serde(default)]
    name: String,
    /// Node adapter or built-in type.
    #[serde(default, rename = "node_type")]
    node_type: String,
    /// Latest runtime state.
    #[serde(default)]
    status: String,
    /// Ordered command parameters.
    #[serde(default)]
    parameters: Vec<String>,
}

/// Complete immediate-mode state for one Diamant window.
struct DiamantApp {
    /// Editable settings shown by the cards.
    settings: GuiSettings,
    /// Current connection-state machine value.
    connection_state: String,
    /// Diagnostic for the current state.
    connection_message: String,
    /// Whether an agent thread owns a run.
    running: bool,
    /// Latest running-node snapshots.
    nodes: Vec<NodeSnapshot>,
    /// Cancellation flag for the current run.
    stop: Option<Arc<AtomicBool>>,
    /// Receiver drained on every frame.
    receiver: Receiver<UiEvent>,
    /// Sender cloned into reporters.
    sender: Sender<UiEvent>,
    /// Exact Figma brand asset uploaded to the GPU on first use.
    brand_texture: Option<TextureHandle>,
    /// Exact Figma empty-state asset uploaded to the GPU on first use.
    empty_texture: Option<TextureHandle>,
}

impl DiamantApp {
    /// Creates a disconnected window and restores persisted settings.
    fn new(receiver: Receiver<UiEvent>, sender: Sender<UiEvent>) -> Self {
        let defaults = DEFAULT_SETTINGS
            .get()
            .cloned()
            .expect("Diamant UI defaults must be initialized");
        Self {
            settings: load_settings().unwrap_or(defaults),
            connection_state: "disconnected".to_owned(),
            connection_message: String::new(),
            running: false,
            nodes: Vec::new(),
            stop: None,
            receiver,
            sender,
            brand_texture: None,
            empty_texture: None,
        }
    }

    /// Applies all agent updates received since the previous frame.
    fn drain_events(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                UiEvent::Connection { state, message } => {
                    self.connection_state = state;
                    self.connection_message = message;
                }
                UiEvent::Nodes(json) => {
                    self.nodes = serde_json::from_str(&json).unwrap_or_default()
                }
                UiEvent::Failed(message) => {
                    self.running = false;
                    self.connection_state = "error".to_owned();
                    self.connection_message = message;
                    self.stop = None;
                }
                UiEvent::Finished => {
                    self.running = false;
                    self.connection_state = "disconnected".to_owned();
                    self.connection_message.clear();
                    self.nodes.clear();
                    self.stop = None;
                }
            }
        }
    }

    /// Starts a validated run or requests deferred shutdown of the active run.
    fn toggle_connection(&mut self, context: &egui::Context) {
        if self.running {
            if let Some(stop) = &self.stop {
                stop.store(true, Ordering::Relaxed);
            }
            self.connection_state = "disconnecting".to_owned();
            return;
        }
        let config = match self.settings.to_config() {
            Ok(config) => config,
            Err(message) => {
                self.connection_state = "error".to_owned();
                self.connection_message = message;
                return;
            }
        };
        if let Err(message) = save_settings(&self.settings) {
            self.connection_state = "error".to_owned();
            self.connection_message = message;
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(Arc::clone(&stop));
        self.running = true;
        self.connection_state = "connecting".to_owned();
        self.connection_message.clear();
        self.nodes.clear();
        let reporter = Reporter {
            sender: self.sender.clone(),
            context: context.clone(),
        };
        let launcher = Arc::clone(
            AGENT_LAUNCHER
                .get()
                .expect("Diamant agent launcher must be initialized"),
        );
        std::thread::spawn(move || launcher(config, stop, reporter));
    }

    /// Renders the fixed application header and connection action.
    fn header(&mut self, ui: &mut Ui) -> f32 {
        let context = ui.ctx().clone();
        let brand_texture = self
            .brand_texture
            .get_or_insert_with(|| {
                load_embedded_texture(
                    &context,
                    "diamant-box",
                    include_bytes!("../assets/diamant-box.png"),
                )
            })
            .clone();
        let width = ui.available_width();
        let response = Frame::NONE
            .fill(Color32::from_rgb(32, 32, 32))
            .inner_margin(Margin::symmetric(24, 0))
            .show(ui, |ui| {
                ui.set_min_width((width - 48.0).max(0.0));
                // The header MUST have a fixed height. A minimum alone lets the
                // nested horizontal layout inherit the full window height.
                ui.set_height(64.0);
                ui.horizontal_centered(|ui| {
                    ui.add_sized(
                        [36.0, 36.0],
                        egui::Button::image(
                            egui::Image::new(&brand_texture).fit_to_exact_size(Vec2::splat(18.0)),
                        )
                        .fill(VIOLET)
                        .stroke(Stroke::NONE)
                        .corner_radius(8.0)
                        .sense(egui::Sense::hover()),
                    );
                    ui.add_space(12.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(190.0, 38.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(
                                RichText::new("Diamant")
                                    .color(Color32::WHITE)
                                    .size(16.0)
                                    .strong(),
                            );
                            if ui.available_width() >= 720.0 {
                                ui.label(
                                    RichText::new("Suntitle host agent")
                                        .color(Color32::from_rgb(173, 173, 173))
                                        .size(12.0),
                                );
                            }
                        },
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if self.running {
                            "Disconnect"
                        } else {
                            "Connect to hub"
                        };
                        let button = egui::Button::new(
                            RichText::new(label)
                                .color(if self.running {
                                    VIOLET_TEXT
                                } else {
                                    Color32::WHITE
                                })
                                .size(14.0),
                        )
                        .fill(if self.running { Color32::WHITE } else { VIOLET })
                        .stroke(if self.running {
                            Stroke::new(1.0, VIOLET)
                        } else {
                            Stroke::NONE
                        })
                        .corner_radius(10.0);
                        if ui.add_sized([121.0, 36.0], button).clicked() {
                            self.toggle_connection(&context);
                        }
                        if ui.available_width() >= 720.0 {
                            ui.add_space(12.0);
                            let (text, fill, color) = self.connection_badge();
                            let response = badge(ui, text, 99.0, fill, color);
                            if !self.connection_message.is_empty() {
                                response.on_hover_text(&self.connection_message);
                            }
                        }
                    });
                });
            });
        response.response.rect.height()
    }

    /// Returns the current badge label and semantic colors.
    fn connection_badge(&self) -> (&str, Color32, Color32) {
        let text = match self.connection_state.as_str() {
            "connected" => "Connected",
            "connecting" => "Connecting…",
            "disconnecting" => "Disconnecting…",
            "error" => "Connection error",
            _ => "Not connected",
        };
        if self.running {
            (text, GREEN_BACKGROUND, GREEN_TEXT)
        } else {
            (text, BACKGROUND, VIOLET_TEXT)
        }
    }

    /// Renders the hub connection card.
    fn hub_card(&mut self, ui: &mut Ui) {
        card(ui, 154.0, |ui| {
            card_heading(
                ui,
                "Hub connection",
                if self.running {
                    "Connected to the Suntitle hub. Changes are saved locally."
                } else {
                    "Configure the hub and host identity, then connect. Settings are saved locally."
                },
                if self.running {
                    "Heartbeat: just now"
                } else {
                    "Ready to connect"
                },
            );
            ui.add_space(12.0);
            let columns = responsive_columns(ui.available_width(), 3);
            let field_width = (ui.available_width() - 12.0 * (columns - 1) as f32) / columns as f32;
            Grid::new("hub_fields")
                .num_columns(columns)
                .spacing(Vec2::new(12.0, 12.0))
                .show(ui, |ui| {
                    field(
                        ui,
                        "Proxy URL",
                        &mut self.settings.proxy_url,
                        "",
                        true,
                        field_width,
                    );
                    next_grid_cell(ui, 1, columns);
                    field(
                        ui,
                        "Host name",
                        &mut self.settings.host_name,
                        "",
                        false,
                        field_width,
                    );
                    next_grid_cell(ui, 2, columns);
                    field(
                        ui,
                        "Host IP",
                        &mut self.settings.host_ip,
                        "Automatic",
                        true,
                        field_width,
                    );
                });
        });
    }

    /// Renders persistent runtime settings.
    fn runtime_card(&mut self, ui: &mut Ui) {
        card(ui, 154.0, |ui| {
            card_heading(
                ui,
                "Runtime settings",
                "These values replace the remaining Diamant CLI arguments.",
                "Saved on this device",
            );
            ui.add_space(12.0);
            let columns = responsive_columns(ui.available_width(), 4);
            let available = ui.available_width() - 8.0 * (columns - 1) as f32;
            let wide = columns == 4;
            let long_width = if wide {
                available * (390.0 / 1080.0)
            } else {
                available / columns as f32
            };
            let short_width = if wide {
                available * (150.0 / 1080.0)
            } else {
                available / columns as f32
            };
            Grid::new("runtime_fields")
                .num_columns(columns)
                .spacing(Vec2::new(8.0, 12.0))
                .show(ui, |ui| {
                    field(
                        ui,
                        "Data-plane URL",
                        &mut self.settings.data_plane_url,
                        "Derived from proxy URL",
                        true,
                        long_width,
                    );
                    next_grid_cell(ui, 1, columns);
                    number_field(
                        ui,
                        "Health interval",
                        &mut self.settings.health_interval,
                        short_width,
                    );
                    next_grid_cell(ui, 2, columns);
                    number_field(
                        ui,
                        "Shutdown grace",
                        &mut self.settings.shutdown_grace,
                        short_width,
                    );
                });
        });
    }

    /// Renders the node table or disconnected empty state.
    fn nodes_card(&mut self, ui: &mut Ui, height: f32) {
        let context = ui.ctx().clone();
        let empty_texture = self
            .empty_texture
            .get_or_insert_with(|| {
                load_embedded_texture(
                    &context,
                    "empty-nodes",
                    include_bytes!("../assets/empty-nodes.png"),
                )
            })
            .clone();
        let width = ui.available_width();
        Frame::NONE
            .fill(CARD)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(8.0)
            .shadow(Shadow {
                offset: [0, 8],
                blur: 20,
                spread: 0,
                color: Color32::from_black_alpha(24),
            })
            .show(ui, |ui| {
                ui.set_width((width - 2.0).max(0.0));
                ui.set_height((height - 2.0).max(0.0));
                ui.spacing_mut().item_spacing.y = 0.0;
                Frame::NONE
                    .inner_margin(Margin::symmetric(24, 0))
                    .show(ui, |ui| {
                        ui.set_width((width - 48.0).max(0.0));
                        ui.set_height(64.0);
                        ui.horizontal_centered(|ui| {
                            ui.allocate_ui_with_layout(
                                Vec2::new(780.0, 40.0),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(
                                        RichText::new("Running nodes")
                                            .color(VIOLET_TEXT)
                                            .size(15.0)
                                            .strong(),
                                    );
                                    ui.label(
                                        RichText::new(if self.running {
                                            "Processes currently managed by this Diamant instance."
                                        } else {
                                            "Connect to the hub to load and manage assigned nodes."
                                        })
                                        .color(MUTED)
                                        .size(12.0),
                                    );
                                },
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                badge(
                                    ui,
                                    &format!("{} running", self.nodes.len()),
                                    70.0,
                                    if self.nodes.is_empty() {
                                        BACKGROUND
                                    } else {
                                        GREEN_BACKGROUND
                                    },
                                    if self.nodes.is_empty() {
                                        VIOLET_TEXT
                                    } else {
                                        GREEN_TEXT
                                    },
                                );
                            });
                        });
                    });
                let (divider, _) =
                    ui.allocate_exact_size(Vec2::new(width, 1.0), egui::Sense::hover());
                ui.painter().rect_filled(divider, 0.0, BORDER);
                if self.nodes.is_empty() {
                    let available_height = ui.available_height();
                    ui.allocate_ui_with_layout(
                        Vec2::new(width, available_height),
                        Layout::top_down(Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 10.0;
                            let empty_state_height = 98.0;
                            ui.add_space(((available_height - empty_state_height) / 2.0).max(0.0));
                            ui.add_sized(
                                [44.0, 44.0],
                                egui::Button::image(
                                    egui::Image::new(&empty_texture)
                                        .fit_to_exact_size(Vec2::splat(20.0)),
                                )
                                .fill(BACKGROUND)
                                .stroke(Stroke::NONE)
                                .corner_radius(8.0)
                                .sense(egui::Sense::hover()),
                            );
                            ui.label(
                                RichText::new("No running nodes")
                                    .color(FOREGROUND)
                                    .size(14.0)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new(if self.running {
                                    "No nodes are currently assigned to this host."
                                } else {
                                    "Connect to the hub to receive node assignments."
                                })
                                .color(MUTED)
                                .size(12.0),
                            );
                        },
                    );
                    return;
                }
                Frame::NONE.inner_margin(Margin::same(24)).show(ui, |ui| {
                    Grid::new("node_table")
                        .striped(true)
                        .num_columns(4)
                        .min_col_width(110.0)
                        .show(ui, |ui| {
                            for title in ["Name", "Type", "Status", "Command parameters"] {
                                ui.label(
                                    RichText::new(title).color(FOREGROUND).size(14.0).strong(),
                                );
                            }
                            ui.end_row();
                            for node in &self.nodes {
                                ui.label(
                                    RichText::new(&node.name)
                                        .color(FOREGROUND)
                                        .size(14.0)
                                        .strong(),
                                );
                                ui.label(
                                    RichText::new(&node.node_type).color(FOREGROUND).size(14.0),
                                );
                                let color = match node.status.as_str() {
                                    "Running" => GREEN_TEXT,
                                    "Failed" => Color32::from_rgb(220, 38, 38),
                                    _ => MUTED,
                                };
                                ui.label(RichText::new(&node.status).color(color).size(14.0));
                                ui.label(
                                    RichText::new(node.parameters.join(" "))
                                        .color(FOREGROUND)
                                        .size(14.0),
                                );
                                ui.end_row();
                            }
                        });
                });
            });
    }
}

impl Drop for DiamantApp {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

impl eframe::App for DiamantApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        Frame::central_panel(ui.style())
            .fill(BACKGROUND)
            .inner_margin(0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.header(ui);
                Frame::NONE.inner_margin(Margin::same(24)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 0.0;
                    self.hub_card(ui);
                    ui.add_space(16.0);
                    self.runtime_card(ui);
                    ui.add_space(16.0);
                    let nodes_height = ui.available_height().max(240.0);
                    self.nodes_card(ui, nodes_height);
                });
            });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BACKGROUND.to_normalized_gamma_f32()
    }
}

/// Chooses one, two, or the requested maximum number of grid columns.
fn responsive_columns(width: f32, maximum: usize) -> usize {
    if width >= 950.0 {
        maximum
    } else if width >= 650.0 {
        2.min(maximum)
    } else {
        1
    }
}

/// Advances a responsive grid after a field at the supplied one-based index.
fn next_grid_cell(ui: &mut Ui, index: usize, columns: usize) {
    if index.is_multiple_of(columns) {
        ui.end_row();
    }
}

/// Renders a fixed-height semantic badge without inheriting its row height.
fn badge(ui: &mut Ui, text: &str, width: f32, fill: Color32, color: Color32) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 0.0;
        ui.add_sized(
            [width, 22.0],
            egui::Button::new(RichText::new(text).color(color).size(12.0).strong())
                .fill(fill)
                .stroke(Stroke::NONE)
                .corner_radius(11.0)
                .sense(egui::Sense::hover()),
        )
    })
    .inner
}

/// Renders a bordered white card matching the approved Diamant design.
fn card(ui: &mut Ui, height: f32, contents: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    Frame::NONE
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(8.0)
        .shadow(Shadow {
            offset: [0, 8],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(24),
        })
        .inner_margin(Margin::symmetric(24, 18))
        .show(ui, |ui| {
            ui.set_min_width((width - 50.0).max(0.0));
            ui.set_min_height((height - 38.0).max(0.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            contents(ui);
        });
}

/// Renders a card title, supporting description, and trailing detail.
fn card_heading(ui: &mut Ui, title: &str, description: &str, detail: &str) {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 38.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).color(VIOLET_TEXT).size(15.0).strong());
                ui.label(RichText::new(description).color(MUTED).size(12.0));
            });
            if ui.available_width() >= 520.0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(detail).color(VIOLET_TEXT).size(12.0).strong());
                });
            }
        },
    );
}

/// Renders one labelled compact text field.
fn field(ui: &mut Ui, label: &str, value: &mut String, hint: &str, editable: bool, width: f32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(RichText::new(label).color(MUTED).size(12.0).strong());
        let edit = TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(width)
            .interactive(editable)
            .align(egui::Align2::LEFT_CENTER)
            .margin(Margin::symmetric(10, 0));
        ui.add_sized([width, 32.0], edit);
    });
}

/// Renders one labelled positive-integer seconds field.
fn number_field(ui: &mut Ui, label: &str, value: &mut u64, width: f32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(RichText::new(label).color(MUTED).size(12.0).strong());
        Frame::NONE
            .fill(CARD)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(10.0)
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.set_width((width - 22.0).max(0.0));
                ui.set_height(22.0);
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size.y = 20.0;
                    ui.visuals_mut().widgets.inactive.bg_fill = CARD;
                    ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
                    ui.visuals_mut().widgets.hovered.bg_fill = CARD;
                    ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
                    ui.visuals_mut().widgets.active.bg_fill = CARD;
                    ui.visuals_mut().widgets.active.bg_stroke = Stroke::NONE;
                    ui.add_sized(
                        [(width - 22.0).max(0.0), 20.0],
                        egui::DragValue::new(value)
                            .range(1..=u64::MAX)
                            .speed(1)
                            .suffix(" s"),
                    );
                });
            });
    });
}

/// Decodes one bundled Figma PNG into an egui texture.
fn load_embedded_texture(context: &egui::Context, name: &str, bytes: &[u8]) -> TextureHandle {
    let image = image::load_from_memory(bytes)
        .expect("bundled Figma asset must be valid PNG")
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    context.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}

/// Returns the cross-platform settings path.
fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("suntitle").join("diamant.json"))
}

/// Loads settings, returning `None` for missing or invalid data.
fn load_settings() -> Option<GuiSettings> {
    serde_json::from_slice(&fs::read(settings_path()?).ok()?).ok()
}

/// Replaces the persisted GUI settings.
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

/// Runs the native Diamant window until the user closes it.
pub fn run<F>(config: AgentConfig, launcher: F) -> ExitCode
where
    F: Fn(AgentConfig, Arc<AtomicBool>, Reporter) + Send + Sync + 'static,
{
    let _ = DEFAULT_SETTINGS.set(GuiSettings::from(&config));
    let _ = AGENT_LAUNCHER.set(Arc::new(launcher));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([560.0, 650.0])
            .with_title("Suntitle — Diamant"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Suntitle — Diamant",
        options,
        Box::new(|creation| {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "Geist".to_owned(),
                egui::FontData::from_static(include_bytes!("../assets/Geist-Variable.ttf")).into(),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "Geist".to_owned());
            creation.egui_ctx.set_fonts(fonts);
            creation.egui_ctx.set_theme(egui::Theme::Light);
            creation.egui_ctx.style_mut_of(egui::Theme::Light, |style| {
                style.visuals = egui::Visuals::light();
                style.visuals.panel_fill = BACKGROUND;
                style.visuals.window_fill = CARD;
                style.visuals.faint_bg_color = BACKGROUND;
                style.visuals.extreme_bg_color = CARD;
                style.visuals.disabled_alpha = 1.0;
                style.visuals.widgets.noninteractive.bg_fill = CARD;
                style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
                style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, FOREGROUND);
                style.visuals.widgets.noninteractive.corner_radius = 8.0.into();
                style.visuals.widgets.inactive.bg_fill = CARD;
                style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
                style.visuals.widgets.inactive.corner_radius = 8.0.into();
                style.visuals.widgets.hovered.bg_fill = CARD;
                style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, VIOLET);
                style.visuals.widgets.hovered.corner_radius = 8.0.into();
                style.visuals.widgets.active.bg_fill = CARD;
                style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, VIOLET);
                style.visuals.widgets.active.corner_radius = 8.0.into();
                style.spacing.interact_size.y = 32.0;
            });
            let (sender, receiver) = mpsc::channel();
            Ok(Box::new(DiamantApp::new(receiver, sender)))
        }),
    );
    if let Err(error) = result {
        eprintln!("failed to run Diamant UI: {error}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns valid settings for focused validation tests.
    fn settings() -> GuiSettings {
        GuiSettings {
            proxy_url: "ws://127.0.0.1:3030/register".to_owned(),
            host_name: "test-host".to_owned(),
            host_ip: String::new(),
            health_interval: 30,
            data_plane_url: String::new(),
            shutdown_grace: 5,
        }
    }

    #[test]
    fn rejects_invalid_host_ip() {
        let mut settings = settings();
        settings.host_ip = "not-an-address".to_owned();
        assert!(
            settings
                .to_config()
                .unwrap_err()
                .starts_with("Invalid host IP:")
        );
    }

    #[test]
    fn trims_values_before_launch() {
        let mut settings = settings();
        settings.host_name = "  test-host  ".to_owned();
        assert_eq!(settings.to_config().unwrap().host_name, "test-host");
    }

    #[test]
    fn header_does_not_consume_the_window_height() {
        let (sender, receiver) = mpsc::channel();
        let mut app = DiamantApp {
            settings: settings(),
            connection_state: "disconnected".to_owned(),
            connection_message: String::new(),
            running: false,
            nodes: Vec::new(),
            stop: None,
            receiver,
            sender,
            brand_texture: None,
            empty_texture: None,
        };
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1200.0, 800.0),
            )),
            ..Default::default()
        };
        let mut header_height = 0.0;

        let mut output = context.run_ui(input, |ui| {
            header_height = app.header(ui);
        });
        output.textures_delta.clear();

        assert_eq!(header_height, 64.0);
    }
}

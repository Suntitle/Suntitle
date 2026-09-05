use std::collections::BTreeMap;
use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

fn sample_rate_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .enum_values(Some([8000, 16000, 22050, 44100, 48000]))
        .into()
}

const fn default_beam_size() -> u8 {
    5
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoginRequest {
    #[schema(min_length = 1, max_length = 128)]
    pub username: String,
    #[schema(format = Password, min_length = 1, max_length = 1024)]
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoginResponse {
    /// Value to use with the `Authorization` header.
    pub access_token: String,
    #[schema(example = "Bearer")]
    pub token_type: String,
    /// Token lifetime in seconds.
    #[schema(minimum = 1)]
    pub expires_in: u64,
    pub user: User,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub id: Uuid,
    pub username: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    AudioSource,
    Transcriber,
    Translator,
    Display,
}

/// A value accepted by a node setting field.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum NodeSettingValue {
    String(String),
    Number(f64),
    Boolean(bool),
    Null,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSettingOption {
    pub value: NodeSettingValue,
    pub label: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NodeSettingInput {
    Text,
    Number,
    Select,
    Checkbox,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NodeSettingValueType {
    String,
    Integer,
    Number,
    Boolean,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeSettingDefinition {
    /// Property name used in the node's settings object.
    #[schema(pattern = "^[A-Za-z][A-Za-z0-9]*$")]
    pub key: String,
    pub label: String,
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[schema(inline)]
    pub input: NodeSettingInput,
    #[schema(inline)]
    pub value_type: NodeSettingValueType,
    pub required: bool,
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<NodeSettingValue>,
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<NodeSettingOption>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeTypeDefinition {
    #[serde(rename = "type")]
    pub node_type: NodeType,
    pub name: String,
    pub description: String,
    /// Node types that may be selected as this node's upstream input.
    pub input_types: Vec<NodeType>,
    pub settings: Vec<NodeSettingDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeTypeDefinitionList {
    pub items: Vec<NodeTypeDefinition>,
    #[schema(minimum = 0)]
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Connected,
    Disconnected,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum HostStatus {
    Online,
    Offline,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Host {
    pub id: Uuid,
    #[schema(min_length = 1, max_length = 128)]
    pub name: String,
    #[schema(value_type = String, format = "ip", examples("192.0.2.10", "2001:db8::10"))]
    pub ip: IpAddr,
    /// Version of the Suntitle host agent or runtime.
    #[schema(min_length = 1, examples("0.1.0"))]
    pub version: String,
    pub status: HostStatus,
    pub last_seen_at: DateTime<Utc>,
    /// Optional host capabilities or implementation-specific information.
    #[schema(additional_properties = true, nullable = false)]
    pub metadata: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HostList {
    pub items: Vec<Host>,
    #[schema(minimum = 0)]
    pub total: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionStatus {
    Connected,
    Reconnecting,
    Degraded,
}

/// Type of data transferred over a node connection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    Audio,
    Transcription,
    Translation,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionProtocol {
    Websocket,
    Http,
    Https,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeConnection {
    /// Stable identifier of this connection.
    pub id: Uuid,
    pub peer_node_id: Uuid,
    pub peer_node_type: NodeType,
    #[schema(nullable = false)]
    pub peer_name: Option<String>,
    #[schema(inline)]
    pub direction: ConnectionDirection,
    pub data_type: DataType,
    pub status: ConnectionStatus,
    #[schema(inline)]
    pub protocol: ConnectionProtocol,
    pub connected_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    #[schema(minimum = 0, nullable = false)]
    pub bytes_transferred: Option<u64>,
    /// Optional implementation-specific connection information.
    #[schema(additional_properties = true, nullable = false)]
    pub metadata: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeBase {
    #[schema(read_only)]
    pub id: Uuid,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    #[schema(min_length = 1, max_length = 128)]
    pub name: String,
    #[schema(nullable, max_length = 1000)]
    pub description: Option<String>,
    pub enabled: bool,
    #[schema(read_only)]
    pub status: NodeStatus,
    #[schema(read_only)]
    pub host: Host,
    #[schema(read_only)]
    pub connections: Vec<NodeConnection>,
    #[schema(read_only)]
    pub created_at: DateTime<Utc>,
    #[schema(read_only)]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeUpdateBase {
    #[serde(rename = "type")]
    pub node_type: NodeType,
    #[schema(min_length = 1, max_length = 128)]
    pub name: String,
    #[schema(nullable, max_length = 1000)]
    pub description: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeInputUpdate {
    /// Upstream node to connect as input. Must have a type included in the
    /// selected node type's `inputTypes`; use null when the type accepts no input.
    #[schema(nullable, required = true)]
    pub input_node_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioSourceSettings {
    #[schema(schema_with = sample_rate_schema)]
    pub sample_rate_hz: u32,
    #[schema(minimum = 1, maximum = 2)]
    pub channels: u8,
    #[schema(inline)]
    pub encoding: AudioEncoding,
    #[schema(nullable, max_length = 256)]
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioEncoding {
    PcmS16le,
    PcmF32le,
    Opus,
}

/// BCP 47 language tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LanguageTag(pub String);

impl utoipa::PartialSchema for LanguageTag {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::String)
            .pattern(Some("^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$"))
            .description(Some("BCP 47 language tag."))
            .examples(["en", "en-US"])
            .into()
    }
}

impl ToSchema for LanguageTag {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TranscriberTask {
    #[default]
    Transcribe,
    Translate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriberSettings {
    #[schema(min_length = 1, examples("large-v3"))]
    pub model: String,
    pub language: LanguageTag,
    #[serde(default)]
    #[schema(default = "transcribe")]
    #[schema(inline)]
    pub task: TranscriberTask,
    #[serde(default = "default_beam_size")]
    #[schema(minimum = 1, maximum = 20, default = 5)]
    pub beam_size: u8,
    #[serde(default = "default_true")]
    #[schema(default = true)]
    pub vad_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslatorSettings {
    pub source_language: LanguageTag,
    pub target_language: LanguageTag,
    #[schema(min_length = 1, examples("openai"))]
    pub provider: String,
    #[schema(nullable)]
    pub model: Option<String>,
    #[serde(default = "default_true")]
    #[schema(default = true)]
    pub preserve_formatting: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DisplayTheme {
    Light,
    #[default]
    Dark,
    Transparent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisplaySettings {
    #[schema(minimum = 1, maximum = 10)]
    pub max_lines: u8,
    #[schema(minimum = 0.5, maximum = 4.0)]
    pub font_scale: f64,
    pub show_original: bool,
    #[serde(default)]
    #[schema(default = "dark")]
    #[schema(inline)]
    pub theme: DisplayTheme,
}

macro_rules! variant {
    ($name:ident, $settings:ty) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
        pub struct $name {
            #[serde(rename = "type")]
            pub node_type: NodeType,
            pub settings: $settings,
        }
    };
}

variant!(AudioSourceVariant, AudioSourceSettings);
variant!(TranscriberVariant, TranscriberSettings);
variant!(TranslatorVariant, TranslatorSettings);
variant!(DisplayVariant, DisplaySettings);

macro_rules! node_shape {
    ($name:ident, $base:ty, $variant:ty) => {
        #[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
        pub struct $name {
            #[serde(flatten)]
            pub base: $base,
            #[serde(flatten)]
            pub variant: $variant,
        }
    };
}

node_shape!(AudioSourceNode, NodeBase, AudioSourceVariant);
node_shape!(TranscriberNode, NodeBase, TranscriberVariant);
node_shape!(TranslatorNode, NodeBase, TranslatorVariant);
node_shape!(DisplayNode, NodeBase, DisplayVariant);

node_shape!(AudioSourceNodeCreate, NodeUpdateBase, AudioSourceVariant);
node_shape!(TranscriberNodeCreate, NodeUpdateBase, TranscriberVariant);
node_shape!(TranslatorNodeCreate, NodeUpdateBase, TranslatorVariant);
node_shape!(DisplayNodeCreate, NodeUpdateBase, DisplayVariant);

macro_rules! update_shape {
    ($name:ident, $variant:ty) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
        pub struct $name {
            #[serde(flatten)]
            pub base: NodeUpdateBase,
            #[serde(flatten)]
            pub variant: $variant,
            #[serde(flatten)]
            pub input: NodeInputUpdate,
        }
    };
}

update_shape!(AudioSourceNodeUpdate, AudioSourceVariant);
update_shape!(TranscriberNodeUpdate, TranscriberVariant);
update_shape!(TranslatorNodeUpdate, TranslatorVariant);
update_shape!(DisplayNodeUpdate, DisplayVariant);

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
#[schema(discriminator(property_name = "type", mapping(
    ("audio_source" = "#/components/schemas/AudioSourceNode"),
    ("transcriber" = "#/components/schemas/TranscriberNode"),
    ("translator" = "#/components/schemas/TranslatorNode"),
    ("display" = "#/components/schemas/DisplayNode")
)))]
pub enum Node {
    AudioSource(AudioSourceNode),
    Transcriber(TranscriberNode),
    Translator(TranslatorNode),
    Display(DisplayNode),
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(untagged)]
#[schema(discriminator(property_name = "type", mapping(
    ("audio_source" = "#/components/schemas/AudioSourceNodeCreate"),
    ("transcriber" = "#/components/schemas/TranscriberNodeCreate"),
    ("translator" = "#/components/schemas/TranslatorNodeCreate"),
    ("display" = "#/components/schemas/DisplayNodeCreate")
)))]
pub enum NodeCreate {
    AudioSource(AudioSourceNodeCreate),
    Transcriber(TranscriberNodeCreate),
    Translator(TranslatorNodeCreate),
    Display(DisplayNodeCreate),
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(untagged)]
#[schema(discriminator(property_name = "type", mapping(
    ("audio_source" = "#/components/schemas/AudioSourceNodeUpdate"),
    ("transcriber" = "#/components/schemas/TranscriberNodeUpdate"),
    ("translator" = "#/components/schemas/TranslatorNodeUpdate"),
    ("display" = "#/components/schemas/DisplayNodeUpdate")
)))]
pub enum NodeUpdate {
    AudioSource(AudioSourceNodeUpdate),
    Transcriber(TranscriberNodeUpdate),
    Translator(TranslatorNodeUpdate),
    Display(DisplayNodeUpdate),
}

impl Node {
    pub fn base(&self) -> &NodeBase {
        match self {
            Self::AudioSource(node) => &node.base,
            Self::Transcriber(node) => &node.base,
            Self::Translator(node) => &node.base,
            Self::Display(node) => &node.base,
        }
    }

    pub fn base_mut(&mut self) -> &mut NodeBase {
        match self {
            Self::AudioSource(node) => &mut node.base,
            Self::Transcriber(node) => &mut node.base,
            Self::Translator(node) => &mut node.base,
            Self::Display(node) => &mut node.base,
        }
    }

    pub fn id(&self) -> Uuid {
        self.base().id
    }

    pub fn node_type(&self) -> NodeType {
        self.base().node_type
    }

    pub fn status(&self) -> NodeStatus {
        self.base().status
    }

    pub fn enabled(&self) -> bool {
        self.base().enabled
    }

    pub fn set_status(&mut self, status: NodeStatus) {
        let base = self.base_mut();
        base.status = status;
        base.updated_at = Utc::now();
    }

    pub fn from_update(id: Uuid, host: Host, update: &NodeUpdate) -> Self {
        let now = Utc::now();
        let update_base = update.base();
        Self::from_base_and_update(
            NodeBase {
                id,
                node_type: update_base.node_type,
                name: update_base.name.clone(),
                description: update_base.description.clone(),
                enabled: update_base.enabled,
                status: NodeStatus::Disconnected,
                host,
                connections: Vec::new(),
                created_at: now,
                updated_at: now,
            },
            update,
        )
    }

    pub fn with_update(&self, update: &NodeUpdate) -> Self {
        let old = self.base();
        let update_base = update.base();
        Self::from_base_and_update(
            NodeBase {
                id: old.id,
                node_type: update_base.node_type,
                name: update_base.name.clone(),
                description: update_base.description.clone(),
                enabled: update_base.enabled,
                status: NodeStatus::Disconnected,
                host: old.host.clone(),
                connections: old.connections.clone(),
                created_at: old.created_at,
                updated_at: Utc::now(),
            },
            update,
        )
    }

    fn from_base_and_update(base: NodeBase, update: &NodeUpdate) -> Self {
        match update {
            NodeUpdate::AudioSource(node) => Self::AudioSource(AudioSourceNode {
                base,
                variant: node.variant.clone(),
            }),
            NodeUpdate::Transcriber(node) => Self::Transcriber(TranscriberNode {
                base,
                variant: node.variant.clone(),
            }),
            NodeUpdate::Translator(node) => Self::Translator(TranslatorNode {
                base,
                variant: node.variant.clone(),
            }),
            NodeUpdate::Display(node) => Self::Display(DisplayNode {
                base,
                variant: node.variant.clone(),
            }),
        }
    }
}

impl NodeUpdate {
    pub fn base(&self) -> &NodeUpdateBase {
        match self {
            Self::AudioSource(node) => &node.base,
            Self::Transcriber(node) => &node.base,
            Self::Translator(node) => &node.base,
            Self::Display(node) => &node.base,
        }
    }

    pub fn node_type(&self) -> NodeType {
        self.base().node_type
    }

    pub fn enabled(&self) -> bool {
        self.base().enabled
    }

    pub fn input_node_id(&self) -> Option<Uuid> {
        match self {
            Self::AudioSource(node) => node.input.input_node_id,
            Self::Transcriber(node) => node.input.input_node_id,
            Self::Translator(node) => node.input.input_node_id,
            Self::Display(node) => node.input.input_node_id,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWire<S> {
    #[serde(rename = "type")]
    node_type: NodeType,
    name: String,
    description: Option<String>,
    enabled: bool,
    settings: S,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateWire<S> {
    #[serde(rename = "type")]
    node_type: NodeType,
    name: String,
    description: Option<String>,
    enabled: bool,
    settings: S,
    #[serde(deserialize_with = "deserialize_nullable_uuid")]
    input_node_id: Option<Uuid>,
}

fn deserialize_nullable_uuid<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Uuid>::deserialize(deserializer)
}

fn create_parts<S>(wire: CreateWire<S>) -> (NodeUpdateBase, S) {
    (
        NodeUpdateBase {
            node_type: wire.node_type,
            name: wire.name,
            description: wire.description,
            enabled: wire.enabled,
        },
        wire.settings,
    )
}

fn update_parts<S>(wire: UpdateWire<S>) -> (NodeUpdateBase, S, NodeInputUpdate) {
    (
        NodeUpdateBase {
            node_type: wire.node_type,
            name: wire.name,
            description: wire.description,
            enabled: wire.enabled,
        },
        wire.settings,
        NodeInputUpdate {
            input_node_id: wire.input_node_id,
        },
    )
}

impl<'de> Deserialize<'de> for NodeCreate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let node_type = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::missing_field("type"))?;

        match node_type {
            "audio_source" => {
                let (base, settings) = create_parts(
                    serde_json::from_value::<CreateWire<AudioSourceSettings>>(value)
                        .map_err(serde::de::Error::custom)?,
                );
                Ok(Self::AudioSource(AudioSourceNodeCreate {
                    variant: AudioSourceVariant {
                        node_type: base.node_type,
                        settings,
                    },
                    base,
                }))
            }
            "transcriber" => {
                let (base, settings) = create_parts(
                    serde_json::from_value::<CreateWire<TranscriberSettings>>(value)
                        .map_err(serde::de::Error::custom)?,
                );
                Ok(Self::Transcriber(TranscriberNodeCreate {
                    variant: TranscriberVariant {
                        node_type: base.node_type,
                        settings,
                    },
                    base,
                }))
            }
            "translator" => {
                let (base, settings) = create_parts(
                    serde_json::from_value::<CreateWire<TranslatorSettings>>(value)
                        .map_err(serde::de::Error::custom)?,
                );
                Ok(Self::Translator(TranslatorNodeCreate {
                    variant: TranslatorVariant {
                        node_type: base.node_type,
                        settings,
                    },
                    base,
                }))
            }
            "display" => {
                let (base, settings) = create_parts(
                    serde_json::from_value::<CreateWire<DisplaySettings>>(value)
                        .map_err(serde::de::Error::custom)?,
                );
                Ok(Self::Display(DisplayNodeCreate {
                    variant: DisplayVariant {
                        node_type: base.node_type,
                        settings,
                    },
                    base,
                }))
            }
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["audio_source", "transcriber", "translator", "display"],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for NodeUpdate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let node_type = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::missing_field("type"))?;

        macro_rules! deserialize_update {
            ($settings:ty, $variant:ident, $shape:ident, $result:ident) => {{
                let (base, settings, input) = update_parts(
                    serde_json::from_value::<UpdateWire<$settings>>(value)
                        .map_err(serde::de::Error::custom)?,
                );
                Ok(Self::$result($shape {
                    variant: $variant {
                        node_type: base.node_type,
                        settings,
                    },
                    base,
                    input,
                }))
            }};
        }

        match node_type {
            "audio_source" => deserialize_update!(
                AudioSourceSettings,
                AudioSourceVariant,
                AudioSourceNodeUpdate,
                AudioSource
            ),
            "transcriber" => deserialize_update!(
                TranscriberSettings,
                TranscriberVariant,
                TranscriberNodeUpdate,
                Transcriber
            ),
            "translator" => deserialize_update!(
                TranslatorSettings,
                TranslatorVariant,
                TranslatorNodeUpdate,
                Translator
            ),
            "display" => {
                deserialize_update!(DisplaySettings, DisplayVariant, DisplayNodeUpdate, Display)
            }
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["audio_source", "transcriber", "translator", "display"],
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeList {
    pub items: Vec<Node>,
    #[schema(minimum = 0)]
    pub total: u64,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct ListHostsQuery {
    /// Return only hosts with this status.
    pub status: Option<HostStatus>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct ListNodesQuery {
    /// Return only nodes of this type.
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub node_type: Option<NodeType>,
    /// Return only nodes with this status.
    pub status: Option<NodeStatus>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcriber_defaults_match_contract() {
        let settings: TranscriberSettings = serde_json::from_value(serde_json::json!({
            "model": "large-v3",
            "language": "en"
        }))
        .expect("valid transcriber settings");

        assert!(matches!(settings.task, TranscriberTask::Transcribe));
        assert_eq!(settings.beam_size, 5);
        assert!(settings.vad_enabled);
    }

    #[test]
    fn settings_reject_unknown_properties() {
        let result = serde_json::from_value::<AudioSourceSettings>(serde_json::json!({
            "sampleRateHz": 16000,
            "channels": 1,
            "encoding": "pcm_s16le",
            "unexpected": true
        }));

        assert!(result.is_err());
    }

    #[test]
    fn node_update_deserializes_the_wire_shape() {
        let update = serde_json::from_value::<NodeUpdate>(serde_json::json!({
            "type": "translator",
            "name": "English to Czech",
            "enabled": true,
            "settings": {
                "sourceLanguage": "en",
                "targetLanguage": "cs",
                "provider": "openai"
            },
            "inputNodeId": null
        }));

        assert!(matches!(update, Ok(NodeUpdate::Translator(_))));
    }
}

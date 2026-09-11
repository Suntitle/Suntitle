mod audio_source;
pub use audio_source::{AudioEncoding, AudioSourceNode, AudioSourceSettings};

mod transcriber;
pub use transcriber::{TranscriberNode, TranscriberSettings, TranscriberTask};

mod translator;
pub use translator::{TranslatorNode, TranslatorSettings};

mod display;
pub use display::{DisplayNode, DisplaySettings, DisplayTheme};

mod language;
pub use language::LanguageTag;

mod node;
pub use node::Node;

use std::collections::BTreeMap;
use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use validator::{Validate, ValidationError, ValidationErrors};

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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema,
)]
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
    /// Setting value submitted when this option is selected.
    pub value: NodeSettingValue,
    /// Display text for this option.
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
    /// Display label for the setting.
    pub label: String,
    /// Help text explaining the setting.
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Form control used to edit the setting.
    #[schema(inline)]
    pub input: NodeSettingInput,
    /// Data type accepted by the setting.
    #[schema(inline)]
    pub value_type: NodeSettingValueType,
    /// Whether a value is required for this setting.
    pub required: bool,
    /// Initial value suggested for the setting.
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<NodeSettingValue>,
    /// Hint shown when the setting has no value.
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Inclusive lower bound for a numeric value.
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    /// Inclusive upper bound for a numeric value.
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    /// Available choices for a selection control.
    #[schema(nullable = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<NodeSettingOption>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeTypeDefinition {
    /// Node type described by this definition.
    #[serde(rename = "type")]
    pub node_type: NodeType,
    /// Display name of the node type.
    pub name: String,
    /// Purpose of this node type.
    pub description: String,
    /// Node types that may be selected as this node's upstream input.
    pub input_types: Vec<NodeType>,
    /// Editable settings supported by this node type.
    pub settings: Vec<NodeSettingDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeTypeDefinitionList {
    /// Node type definitions returned by the query.
    pub items: Vec<NodeTypeDefinition>,
    /// Total number of matching node type definitions.
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
    /// Stable identifier of the host.
    pub id: Uuid,
    /// Display name shown for this host.
    #[schema(min_length = 1, max_length = 128)]
    pub name: String,
    /// IP address of the host.
    #[schema(value_type = String, format = "ip", examples("192.0.2.10", "2001:db8::10"))]
    pub ip: IpAddr,
    /// Version of the Suntitle host agent or runtime.
    #[schema(min_length = 1, examples("0.1.0"))]
    pub version: String,
    /// Current host availability and health.
    pub status: HostStatus,
    /// Time the host was last observed by the control plane.
    pub last_seen_at: DateTime<Utc>,
    /// Optional host capabilities or implementation-specific information.
    #[schema(additional_properties = true, nullable = false)]
    pub metadata: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HostList {
    /// Hosts returned by the query.
    pub items: Vec<Host>,
    /// Total number of matching hosts.
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
    /// Identifier of the node at the other end of the connection.
    pub peer_node_id: Uuid,
    /// Processing role of the peer node.
    pub peer_node_type: NodeType,
    /// Display name of the peer node, when known.
    #[schema(nullable = false)]
    pub peer_name: Option<String>,
    /// Direction of data flow relative to this node.
    #[schema(inline)]
    pub direction: ConnectionDirection,
    /// Kind of payload carried by the connection.
    pub data_type: DataType,
    /// Current connection health.
    pub status: ConnectionStatus,
    /// Transport protocol used by the connection.
    #[schema(inline)]
    pub protocol: ConnectionProtocol,
    /// Time the connection was established.
    pub connected_at: DateTime<Utc>,
    /// Time data was last observed on the connection.
    pub last_activity_at: DateTime<Utc>,
    /// Reported number of bytes transferred over the connection.
    #[schema(minimum = 0, nullable = false)]
    pub bytes_transferred: Option<u64>,
    /// Optional implementation-specific connection information.
    #[schema(additional_properties = true, nullable = false)]
    pub metadata: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeBase {
    /// Stable identifier of the node.
    #[schema(read_only)]
    pub id: Uuid,
    /// Display name shown to users.
    #[schema(min_length = 1, max_length = 128)]
    pub name: String,
    /// Optional description of the node’s purpose.
    #[schema(nullable, max_length = 1000)]
    pub description: Option<String>,
    /// Whether the node should run.
    pub enabled: bool,
    /// Current node connectivity and health.
    #[schema(read_only)]
    pub status: NodeStatus,
    /// Host assigned to run this node.
    #[schema(read_only)]
    pub host: Host,
    /// Observed data connections to peer nodes.
    #[schema(read_only)]
    pub connections: Vec<NodeConnection>,
    /// Time the node was created.
    #[schema(read_only)]
    pub created_at: DateTime<Utc>,
    /// Time the node record was last updated.
    #[schema(read_only)]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct NodeUpdateBase {
    /// Display name shown to users.
    #[schema(min_length = 1, max_length = 128)]
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    /// Optional description of the node’s purpose.
    #[schema(nullable, max_length = 1000)]
    #[validate(length(max = 1000))]
    pub description: Option<String>,
    /// Whether the node should run.
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NodeInputUpdate {
    /// Upstream node to connect as input. Must have a type included in the
    /// selected node type's `inputTypes`; use null when the type accepts no input.
    #[schema(nullable, required = true)]
    #[serde(deserialize_with = "deserialize_nullable_uuid")]
    pub input_node_id: Option<Uuid>,
}

/// The node type and its corresponding settings share a single discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "settings", rename_all = "snake_case")]
pub enum NodeSettings {
    AudioSource(AudioSourceSettings),
    Transcriber(TranscriberSettings),
    Translator(TranslatorSettings),
    Display(DisplaySettings),
}

impl NodeSettings {
    pub fn node_type(&self) -> NodeType {
        match self {
            Self::AudioSource(_) => NodeType::AudioSource,
            Self::Transcriber(_) => NodeType::Transcriber,
            Self::Translator(_) => NodeType::Translator,
            Self::Display(_) => NodeType::Display,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
pub struct NodeCreate {
    /// Initial node configuration and upstream input.
    #[serde(flatten)]
    #[validate(nested)]
    pub update: NodeUpdate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Validate)]
pub struct NodeUpdate {
    /// Editable name, description, and enabled state.
    #[serde(flatten)]
    #[validate(nested)]
    pub base: NodeUpdateBase,
    /// Node type discriminator and its matching settings.
    #[serde(flatten)]
    #[validate(nested)]
    pub settings: NodeSettings,
    /// Upstream input selection for the node.
    #[serde(flatten)]
    pub input: NodeInputUpdate,
}

impl NodeUpdate {
    pub fn base(&self) -> &NodeUpdateBase {
        &self.base
    }

    pub fn node_type(&self) -> NodeType {
        self.settings.node_type()
    }

    pub fn enabled(&self) -> bool {
        self.base().enabled
    }

    pub fn input_node_id(&self) -> Option<Uuid> {
        self.input.input_node_id
    }
}

// A custom field deserializer makes absence an error while still accepting null.
fn deserialize_nullable_uuid<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Uuid>::deserialize(deserializer)
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeList {
    /// Nodes returned by the query.
    pub items: Vec<Node>,
    /// Total number of matching nodes.
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

fn require(valid: bool, message: &'static str) -> Result<(), ValidationError> {
    if valid {
        Ok(())
    } else {
        Err(ValidationError::new("unsupported").with_message(message.into()))
    }
}

impl Validate for NodeSettings {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            Self::AudioSource(settings) => settings.validate(),
            Self::Transcriber(settings) => settings.validate(),
            Self::Translator(settings) => settings.validate(),
            Self::Display(settings) => settings.validate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_node_responses_support_all_variants() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mockserver/api/v1");
        let mut tags = std::collections::BTreeSet::new();
        for path in [
            "nodes.json",
            "hosts/20000000-0000-4000-8000-000000000001/nodes.json",
            "nodes/30000000-0000-4000-8000-000000000001.json",
            "nodes/30000000-0000-4000-8000-000000000003.json",
        ] {
            let fixture: Value =
                serde_json::from_str(&std::fs::read_to_string(root.join(path)).unwrap()).unwrap();
            let nodes = fixture
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![fixture]);
            for wire in nodes {
                let node: crate::kubernetes::resources::SuntitleNode =
                    serde_json::from_value(wire.clone()).unwrap();
                let settings = serde_json::to_value(&node.spec.settings).unwrap();
                let expected: crate::kubernetes::resources::NodeSettings =
                    serde_json::from_value(wire["spec"]["settings"].clone()).unwrap();
                assert_eq!(settings, serde_json::to_value(expected).unwrap());
                tags.extend(settings.as_object().unwrap().keys().cloned());
            }
        }
        assert_eq!(
            tags,
            ["audio_source", "transcriber", "translator", "display"]
                .map(str::to_owned)
                .into()
        );
    }

    #[test]
    fn tagged_nodes_preserve_the_client_wire_format() {
        let cases = [
            serde_json::json!({"type":"audio_source","settings":{"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le","deviceName":null}}),
            serde_json::json!({"type":"transcriber","settings":{"model":"large-v3","language":"en","task":"transcribe","beamSize":5,"vadEnabled":true}}),
            serde_json::json!({"type":"translator","settings":{"sourceLanguage":"en","targetLanguage":"cs","provider":"openai","model":null,"preserveFormatting":true}}),
            serde_json::json!({"type":"display","settings":{"maxLines":3,"fontScale":1.0,"showOriginal":false,"theme":"dark"}}),
        ];
        for mut wire in cases {
            wire["name"] = "test".into();
            wire["description"] = Value::Null;
            wire["enabled"] = true.into();
            assert!(serde_json::from_value::<NodeCreate>(wire.clone()).is_err());

            // Required nullable input: omission must fail, null and UUID must work.
            assert!(serde_json::from_value::<NodeUpdate>(wire.clone()).is_err());
            for input in [Value::Null, serde_json::json!(Uuid::new_v4())] {
                wire["inputNodeId"] = input;
                let create: NodeCreate = serde_json::from_value(wire.clone()).unwrap();
                assert_eq!(serde_json::to_value(create).unwrap(), wire);
                let update: NodeUpdate = serde_json::from_value(wire.clone()).unwrap();
                let json = serde_json::to_string(&update).unwrap();
                assert_eq!(json.matches("\"type\"").count(), 1);
                assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), wire);
                assert_eq!(serde_json::from_str::<NodeUpdate>(&json).unwrap(), update);

                let host = Host {
                    id: Uuid::new_v4(),
                    name: "host".into(),
                    ip: "192.0.2.10".parse().unwrap(),
                    version: "0.1.0".into(),
                    status: HostStatus::Online,
                    last_seen_at: Utc::now(),
                    metadata: None,
                };
                let node = Node::from_update(Uuid::new_v4(), host, &update);
                let json = serde_json::to_string(&node).unwrap();
                assert_eq!(json.matches("\"type\"").count(), 1);
                let restored: Node = serde_json::from_str(&json).unwrap();
                let restored_wire = serde_json::to_value(&restored).unwrap();
                assert_eq!(restored_wire["settings"], wire["settings"]);
                assert_eq!(restored_wire["type"], wire["type"]);
                assert_eq!(restored.node_type(), update.node_type());
                assert_eq!(restored.id(), node.id());
            }

            wire["type"] = "unknown".into();
            assert!(serde_json::from_value::<NodeCreate>(wire.clone()).is_err());
            assert!(serde_json::from_value::<NodeUpdate>(wire.clone()).is_err());
            wire["type"] = "display".into();
            wire["settings"] =
                serde_json::json!({"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le"});
            assert!(serde_json::from_value::<NodeCreate>(wire.clone()).is_err());
            assert!(serde_json::from_value::<NodeUpdate>(wire).is_err());
        }
    }

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

        assert!(matches!(
            update.unwrap().settings,
            NodeSettings::Translator(_)
        ));
    }
}

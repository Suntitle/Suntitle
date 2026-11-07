mod audio_source;
pub use audio_source::{AudioEncoding, AudioSourceSettings};

mod transcriber;
pub use transcriber::{TranscriberSettings, TranscriberTask};

mod translator;
pub use translator::TranslatorSettings;

mod display;
pub use display::{DisplaySettings, DisplayTheme};

mod language;
pub use language::LanguageTag;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::ValidationError;

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

/// Built-in pipeline type or custom execution variant. Extension icons use a separate category.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    ToSchema,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    /// Produces audio for the pipeline.
    AudioSource,
    /// Transcribes audio into text.
    Transcriber,
    /// Translates text between languages.
    Translator,
    /// Presents pipeline output.
    Display,
    /// Execution variant for nodes that reference a custom extension.
    #[default]
    Custom,
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
    pub node_type: NodeRole,
    /// Display name of the node type.
    pub name: String,
    /// Purpose of this node type.
    pub description: String,
    /// Node types that may be selected as this node's upstream input.
    pub input_types: Vec<NodeRole>,
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

fn require(valid: bool, message: &'static str) -> Result<(), ValidationError> {
    if valid {
        Ok(())
    } else {
        Err(ValidationError::new("unsupported").with_message(message.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn mock_node_responses_support_all_variants() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mockserver/api/v1");
        let mut tags = std::collections::BTreeSet::new();
        for path in [
            "nodes.json",
            "hosts/studio-host-resource/nodes.json",
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
                let spec: crate::kubernetes::resources::SuntitleNodeSpec =
                    serde_json::from_value(wire.clone()).unwrap();
                let settings = serde_json::to_value(&spec.settings).unwrap();
                let expected: crate::kubernetes::resources::NodeSettings =
                    serde_json::from_value(wire["settings"].clone()).unwrap();
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
}

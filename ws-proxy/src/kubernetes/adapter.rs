use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::api::models::NodeType;

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionAdapter {
    #[serde(rename = "type")]
    pub node_type: NodeType,
    #[schemars(schema_with = "super::resources::reported_metadata_schema")]
    pub execution: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, ExtensionSetting>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<ArgumentBinding>,
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionSetting {
    /// Explicit null MUST remain distinct from an omitted default.
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(schema_with = "arbitrary_value_schema")]
    pub default: Option<Value>,
    #[serde(default)]
    pub required: bool,
}

fn present_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

fn arbitrary_value_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    // CRDs MUST preserve arbitrary values. OpenAPI MUST describe all JSON types.
    let schema = if generator.settings().definitions_path
        == schemars::generate::SchemaSettings::openapi3().definitions_path
    {
        serde_json::json!({"nullable": true, "x-kubernetes-preserve-unknown-fields": true})
    } else {
        serde_json::json!({"type": ["null", "boolean", "object", "array", "number", "string"]})
    };
    schema.try_into().unwrap()
}

#[derive(kube::KubeSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[x_kube(cel)]
#[x_kube(validation = kube::core::Rule::new("!has(self.mode) || self.mode != 'switch' || (has(self.flag) && self.flag != null)")
    .message("switch arguments require a flag"))]
pub struct ArgumentBinding {
    /// Omit the flag for a positional value.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub flag: Option<String>,
    #[schemars(regex(pattern = r"^(runtime\.websocketUrl|settings\..+)$"))]
    pub source: String,
    #[serde(default, skip_serializing_if = "ArgumentMode::is_value")]
    #[schemars(extend("default" = "value"))]
    pub mode: ArgumentMode,
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArgumentMode {
    #[default]
    Value,
    Switch,
}

impl ArgumentMode {
    fn is_value(&self) -> bool {
        matches!(self, Self::Value)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAdapter {
    #[serde(rename = "type")]
    pub node_type: NodeType,
    pub execution: BTreeMap<String, Value>,
    pub settings: BTreeMap<String, Value>,
    pub arguments: Vec<ArgumentBinding>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct MissingRequiredSettings(pub Vec<String>);

impl std::fmt::Display for MissingRequiredSettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "missing required extension settings: {}",
            self.0.join(", ")
        )
    }
}

impl std::error::Error for MissingRequiredSettings {}

impl ExtensionAdapter {
    pub fn resolve(
        &self,
        overrides: &BTreeMap<String, Value>,
    ) -> Result<ResolvedAdapter, MissingRequiredSettings> {
        let mut settings = overrides.clone();
        let mut missing = Vec::new();
        for (name, definition) in &self.settings {
            if settings.contains_key(name) {
                continue;
            }
            if let Some(default) = &definition.default {
                settings.insert(name.clone(), default.clone());
            } else if definition.required {
                missing.push(name.clone());
            }
        }
        if !missing.is_empty() {
            return Err(MissingRequiredSettings(missing));
        }
        let arguments = self
            .arguments
            .iter()
            .filter(|binding| {
                let Some(name) = binding.source.strip_prefix("settings.") else {
                    return true;
                };
                !self.settings.contains_key(name) || settings.contains_key(name)
            })
            .cloned()
            .collect();
        Ok(ResolvedAdapter {
            node_type: self.node_type,
            execution: self.execution.clone(),
            settings,
            arguments,
        })
    }
}

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::api::models::NodeType;

/// Standard adapter fields shared by an extension and its resolved node instance.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[serde(bound(deserialize = "S: Deserialize<'de> + Default"))]
pub struct Adapter<S: Default> {
    /// Pipeline role defining communication and input compatibility.
    #[serde(rename = "type")]
    pub node_type: NodeType,
    /// Typed execution contract, unchanged by plugin setting resolution.
    pub execution: super::execution::ExecutionSpec,
    /// User-defined setting declarations or resolved instance values.
    #[serde(default)]
    pub settings: S,
    /// Ordered mappings from typed sources to command arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<ArgumentBinding>,
}

/// Extension declarations use setting definitions with optional defaults.
pub type ExtensionAdapter = Adapter<BTreeMap<String, ExtensionSetting>>;
/// Resolved node adapters reuse the same contract with actual plugin values.
pub type ResolvedAdapter = Adapter<PluginSettings>;

/// Only plugin-owned values remain dynamic. Keys and value shapes are user-defined.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(transparent)]
pub struct PluginSettings(
    /// Arbitrary JSON values preserved in Kubernetes and API schemas.
    #[schemars(schema_with = "super::resources::reported_metadata_schema")]
    pub BTreeMap<String, Value>,
);
impl std::ops::Deref for PluginSettings {
    type Target = BTreeMap<String, Value>;
    /// Expose user-owned keys without duplicating map operations.
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for PluginSettings {
    /// Allow defaults to fill absent keys while preserving explicit JSON values.
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// One plugin setting declaration. Values may have any user-defined JSON shape.
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
    /// Require an instance value when no default is declared.
    #[serde(default)]
    pub required: bool,
}

/// Preserve an explicit null default instead of treating it as an absent field.
fn present_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// Preserve user-owned JSON values in both Kubernetes and API schema dialects.
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

/// Ordered CLI binding reused before and after node setting resolution.
#[derive(kube::KubeSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[x_kube(cel)]
#[x_kube(validation = kube::core::Rule::new("!has(self.mode) || self.mode != 'switch' || (has(self.flag) && size(self.flag) > 0)")
    .message("switch arguments require a flag"))]
pub struct ArgumentBinding {
    /// Omit the flag for a positional value.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub flag: Option<String>,
    /// Standard runtime value or exact user-defined setting key.
    pub source: ArgumentSource,
    /// Convert to a scalar value or a boolean switch. Switches MUST have a nonempty flag.
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
    /// Overlay node values on defaults without changing standard execution or argument contracts.
    pub fn resolve(
        &self,
        overrides: &PluginSettings,
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
                let ArgumentSource::Setting(name) = &binding.source else {
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

/// Standard runtime values available independently of plugin settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeValue {
    /// Full WebSocket URL selected for the extension's pipeline type.
    WebsocketUrl,
}

/// Typed argument input. The wire format remains runtime.websocketUrl or settings.<key>.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub enum ArgumentSource {
    /// Reserved, statically defined value supplied by Diamant.
    Runtime(RuntimeValue),
    /// Exact key in the user-defined plugin settings object.
    Setting(String),
}
impl TryFrom<String> for ArgumentSource {
    type Error = &'static str;
    /// Reject unknown runtime fields and empty user setting keys at deserialization.
    fn try_from(source: String) -> Result<Self, Self::Error> {
        if source == "runtime.websocketUrl" {
            return Ok(Self::Runtime(RuntimeValue::WebsocketUrl));
        }
        if let Some(key) = source
            .strip_prefix("settings.")
            .filter(|key| !key.is_empty())
        {
            return Ok(Self::Setting(key.into()));
        }
        Err("argument source MUST be runtime.websocketUrl or settings.<key>")
    }
}
impl From<ArgumentSource> for String {
    /// Preserve existing API and YAML binding syntax without runtime string parsing.
    fn from(source: ArgumentSource) -> Self {
        match source {
            ArgumentSource::Runtime(RuntimeValue::WebsocketUrl) => "runtime.websocketUrl".into(),
            ArgumentSource::Setting(key) => format!("settings.{key}"),
        }
    }
}
impl JsonSchema for ArgumentSource {
    /// Name reused by every declared and resolved argument binding schema.
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ArgumentSource".into()
    }
    /// Constrain the wire string while plugin-owned setting keys remain open.
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::json!({"type":"string","pattern":r"^(runtime\.websocketUrl|settings\..+)$"})
            .try_into()
            .unwrap()
    }
}

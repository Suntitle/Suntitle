use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Media type declared by a custom extension. MUST NOT select runtime or endpoint behavior.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionDataType {
    /// Image content.
    Image,
    /// Text content.
    Text,
    /// Audio content.
    Audio,
}

/// Standard adapter fields shared by an extension and its resolved node instance.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[serde(bound(deserialize = "S: Deserialize<'de> + Default"))]
pub struct Adapter<S: Default> {
    /// Optional input media type. Omission means no declared input, without selecting runtime behavior.
    #[serde(
        default,
        deserialize_with = "declared_media_type",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ExtensionDataType")]
    pub input_type: Option<ExtensionDataType>,
    /// Optional output media type. Omission means no declared output, without selecting runtime behavior.
    #[serde(
        default,
        deserialize_with = "declared_media_type",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "ExtensionDataType")]
    pub output_type: Option<ExtensionDataType>,
    /// Typed execution contract, unchanged by plugin setting resolution.
    pub execution: super::execution::ExecutionSpec,
    /// User-defined setting declarations or resolved instance values.
    #[serde(default)]
    pub settings: S,
    /// Ordered mappings from typed sources to command arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<ArgumentBinding>,
}

/// A present media type MUST be a supported string. Only omission represents no declaration.
fn declared_media_type<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ExtensionDataType>, D::Error> {
    ExtensionDataType::deserialize(deserializer).map(Some)
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
// Schemars MUST leave the flattened enum open so Kubernetes can hoist its properties.
#[schemars(!deny_unknown_fields)]
#[x_kube(cel)]
#[x_kube(validation = kube::core::Rule::new("!has(self.mode) || self.mode != 'flag' || has(self.flag)")
    .message("flag mode requires a flag"))]
#[x_kube(validation = kube::core::Rule::new("!has(self.url) || !has(self.mode) || self.mode == 'value'")
    .message("url arguments require value mode"))]
pub struct ArgumentBinding {
    /// Omit the flag for a positional value.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub flag: Option<String>,
    /// Exactly one direct source or structured URL, flattened into the wire object.
    #[serde(flatten)]
    pub value: ArgumentValue,
    /// Convert to a scalar value or boolean flag. URLs MUST use value mode.
    #[serde(default, skip_serializing_if = "ArgumentMode::is_value")]
    #[schemars(extend("default" = "value"))]
    pub mode: ArgumentMode,
}

/// Mutually exclusive argument inputs. The wire keys remain source and url.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ArgumentValue {
    /// Read one runtime value or user setting without interpolation.
    Source(ArgumentSource),
    /// Compose a runtime-owned base URL and a setting-owned endpoint.
    Url(ArgumentUrl),
}

/// Runtime-owned URL origin with a user-owned path. Media types MUST NOT affect composition.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArgumentUrl {
    /// Diamant's configured base URL. No user-defined origin is accepted.
    pub base: RuntimeValue,
    /// Setting reference containing the endpoint path to append.
    pub path: SettingReference,
}

/// Exact nonempty setting key, represented on the wire as settings.<key>.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct SettingReference(
    /// User-owned map key. MUST be nonempty.
    String,
);

impl SettingReference {
    /// Return the exact setting key without its wire prefix.
    pub fn key(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SettingReference {
    type Error = &'static str;

    /// Restrict URL paths to setting references during deserialization.
    fn try_from(source: String) -> Result<Self, Self::Error> {
        match ArgumentSource::try_from(source)? {
            ArgumentSource::Setting(key) => Ok(Self(key)),
            ArgumentSource::Runtime(_) => Err("url.path MUST reference settings.<key>"),
        }
    }
}

impl From<SettingReference> for String {
    /// Preserve the public reference syntax during serialization.
    fn from(source: SettingReference) -> Self {
        format!("settings.{}", source.0)
    }
}

impl JsonSchema for SettingReference {
    /// Reuse one schema identity for exact setting references.
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SettingReference".into()
    }

    /// Exclude runtime values and empty setting names from path references.
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::json!({"type":"string","pattern":r"^settings\..+$"})
            .try_into()
            .unwrap()
    }
}

/// Controls whether an argument emits a scalar value or a boolean-controlled flag.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArgumentMode {
    /// Emit a scalar value with an optional flag.
    #[default]
    Value,
    /// Emit only the flag when its boolean source is true.
    Flag,
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
                let name = match &binding.value {
                    ArgumentValue::Source(ArgumentSource::Setting(name)) => name.as_str(),
                    ArgumentValue::Url(url) => url.path.key(),
                    ArgumentValue::Source(ArgumentSource::Runtime(_)) => return true,
                };
                !self.settings.contains_key(name) || settings.contains_key(name)
            })
            .cloned()
            .collect();
        Ok(ResolvedAdapter {
            input_type: self.input_type,
            output_type: self.output_type,
            execution: self.execution.clone(),
            settings,
            arguments,
        })
    }
}

/// Standard runtime values available independently of plugin settings.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeValue {
    /// Configured data-plane base WebSocket URL, independent of the node role.
    #[serde(rename = "runtime.websocketUrl")]
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

//! Shared desired-state and runtime observation contracts.
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::{CustomResource, CustomResourceExt, KubeSchema};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::models::{
    AudioSourceSettings, DisplaySettings, TranscriberSettings, TranslatorSettings,
};

pub const API_GROUP: &str = "control.suntitle.io";
pub const DEFAULT_NAMESPACE: &str = "suntitle";
pub const NODE_FINALIZER: &str = "control.suntitle.io/runtime-cleanup";
pub const HOST_FINALIZER: &str = "control.suntitle.io/host-cleanup";

/// Kubernetes addressing plus incarnation identity for future runtime messages.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ResourceIdentity {
    /// Kubernetes namespace containing the resource.
    pub namespace: String,
    /// Kubernetes resource name within the namespace.
    pub name: String,
    /// Kubernetes UID distinguishing this resource incarnation.
    pub uid: String,
}

#[derive(CustomResource, JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[kube(
    group = "control.suntitle.io",
    version = "v1alpha1",
    kind = "SuntitleHost",
    namespaced,
    status = "HostStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct SuntitleHostSpec {
    /// Display name shown for this host.
    #[schemars(length(min = 1, max = 128))]
    pub display_name: String,
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRef {
    /// Name of the referenced resource in the same namespace.
    #[schemars(
        length(min = 1, max = 253),
        regex(pattern = "^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$")
    )]
    pub name: String,
}

/// Desired source, adapter, and publication attempt limit for an extension.
#[derive(CustomResource, JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[kube(
    group = "control.suntitle.io",
    version = "v1alpha1",
    kind = "SuntitleExtension",
    namespaced,
    status = "ExtensionStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct SuntitleExtensionSpec {
    /// Display name of the extension.
    #[schemars(length(min = 1, max = 128))]
    pub display_name: String,
    /// Kind of source prepared by the publication worker.
    pub source_type: ExtensionSourceType,
    /// Source path or URL interpreted according to source type.
    #[schemars(length(min = 1))]
    pub source: String,
    /// Opaque token whose change requests extension preparation.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 128))]
    pub refresh_token: Option<String>,
    /// Maximum total publication attempts, including the initial execution. Defaults to three.
    #[serde(default = "publication_attempts_default")]
    #[schemars(range(min = 1))]
    pub max_attempts: u32,
    /// Execution and binding contract packaged with the source.
    pub adapter: super::adapter::ExtensionAdapter,
}

/// Default total attempt limit delegated to Apalis.
fn publication_attempts_default() -> u32 {
    3
}

/// Transport used to discover and publish extension payloads.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionSourceType {
    /// Directory accessible to the server and publication worker.
    Local,
    /// HTTP(S) Git repository.
    Git,
    /// OCI artifact reference.
    Oci,
    /// HTTP(S) payload with a sibling manifest.
    Http,
}

impl JsonSchema for ExtensionSourceType {
    /// Preserve the established schema identity across resource and API schemas.
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ExtensionSourceType".into()
    }

    /// Keep enum constraints flat; variant descriptions MUST NOT create overlapping branches.
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::json!({"type": "string", "enum": [Self::Local, Self::Git, Self::Oci, Self::Http]})
            .try_into()
            .unwrap()
    }
}

/// Publication progress and terminal outcome reported by Apalis for the observed revision.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionStatus {
    /// Resource generation whose publication state is reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// Stable Apalis task identifier for this desired revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Total attempts reported by Apalis once this publication completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<usize>,
    /// Verified digest reference available after successful publication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_reference: Option<String>,
    /// Preparing, published, or terminal failure state; retries are owned by Apalis.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "conditions_schema")]
    pub conditions: Vec<Condition>,
}

#[derive(CustomResource, KubeSchema, Serialize, Deserialize, Clone, Debug)]
#[kube(
    group = "control.suntitle.io",
    version = "v1alpha1",
    kind = "SuntitleNode",
    cel,
    namespaced,
    status = "NodeStatus"
)]
#[serde(rename_all = "camelCase")]
#[x_kube(cel)]
#[x_kube(validation = Rule::new("(has(self.extensionRef) && self.extensionRef != null) == has(self.settings.custom)")
    .message("extensionRef is required for custom settings and forbidden for built-in settings"))]
pub struct SuntitleNodeSpec {
    /// Display name shown to users.
    #[schemars(length(min = 1, max = 128))]
    pub display_name: String,
    /// Optional description of the node’s purpose.
    #[schemars(length(max = 1000))]
    pub description: Option<String>,
    /// Host assigned to run this node. MUST remain unchanged.
    #[x_kube(validation = Rule::new("self == oldSelf").message("host reference is immutable"))]
    pub host_ref: ResourceRef,
    /// Whether the node should run.
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    /// Upstream node supplying input, when configured.
    pub input_ref: Option<ResourceRef>,
    /// Extension in the same namespace supplying custom execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_ref: Option<ResourceRef>,
    /// Opaque token whose change requests a node restart.
    #[schemars(length(min = 1, max = 128))]
    pub restart_token: Option<String>,
    /// Configuration for exactly one node type. The selected type MUST remain unchanged.
    #[x_kube(validation = Rule::new(concat!(
        "has(self.audio_source) == has(oldSelf.audio_source) && ",
        "has(self.transcriber) == has(oldSelf.transcriber) && ",
        "has(self.translator) == has(oldSelf.translator) && ",
        "has(self.display) == has(oldSelf.display) && ",
        "has(self.custom) == has(oldSelf.custom)",
    )).message("node type is immutable"))]
    pub settings: NodeSettings,
}

/// Runnable equality includes CEL-immutable host assignment and the settings variant.
/// Display metadata and pipeline inputs MUST NOT affect runnable equality.
impl PartialEq for SuntitleNodeSpec {
    /// Compare typed process configuration without serializing it to JSON.
    fn eq(&self, other: &Self) -> bool {
        self.host_ref == other.host_ref
            && self.enabled == other.enabled
            && self.settings == other.settings
            && self.extension_ref == other.extension_ref
            && self.restart_token == other.restart_token
    }
}

impl SuntitleNodeSpec {
    pub fn resolve_extension(
        &self,
        extension: &SuntitleExtensionSpec,
    ) -> Result<Option<super::adapter::ResolvedAdapter>, super::adapter::MissingRequiredSettings>
    {
        match &self.settings {
            NodeSettings::Custom(overrides) => extension.adapter.resolve(overrides).map(Some),
            _ => Ok(None),
        }
    }
}

fn enabled_default() -> bool {
    true
}

impl SuntitleHost {
    pub fn conditions(&self) -> &[Condition] {
        self.status
            .as_ref()
            .map(|status| status.conditions.as_slice())
            .unwrap_or_default()
    }
}

impl SuntitleNode {
    pub fn conditions(&self) -> &[Condition] {
        self.status
            .as_ref()
            .map(|status| status.conditions.as_slice())
            .unwrap_or_default()
    }
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    /// IP address reported by the host agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Version of the reporting host agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    /// Capabilities and implementation-specific data reported by the host.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(schema_with = "reported_metadata_schema")]
    pub reported_metadata: BTreeMap<String, Value>,
    /// Time of the latest host heartbeat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<DateTime<Utc>>,
    /// Active host connection session ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Observed host availability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence: Option<Presence>,
    /// Host health observations keyed by condition type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "conditions_schema")]
    pub conditions: Vec<Condition>,
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
pub enum Presence {
    Online,
    Offline,
}

impl Presence {
    pub fn ready_condition(&self, generation: i64) -> Condition {
        Condition {
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: Some(generation),
            type_: "Ready".into(),
            status: match self {
                Self::Online => "True".into(),
                Self::Offline => "False".into(),
            },
            reason: match self {
                Self::Online => "Connected".into(),
                Self::Offline => "Disconnected".into(),
            },
            message: match self {
                Self::Online => "Host session is active.".into(),
                Self::Offline => "Fresh host registration is required.".into(),
            },
        }
    }
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
pub enum NodePhase {
    Pending,
    Running,
    Disabled,
    Failed,
    Disconnected,
    Terminating,
}

impl NodePhase {
    pub fn ready_condition(&self, generation: i64, message: Option<&str>) -> Condition {
        self.condition("Ready", generation, message)
    }

    pub fn updated_condition(&self, generation: i64, message: Option<&str>) -> Condition {
        self.condition("Updated", generation, message)
    }

    fn condition(&self, type_: &str, generation: i64, message: Option<&str>) -> Condition {
        let updated = type_ == "Updated";
        let (status, reason, default_message) = match self {
            Self::Running | Self::Disabled => (
                if updated || matches!(self, Self::Running) {
                    "True"
                } else {
                    "False"
                },
                "DesiredStateApplied",
                if updated {
                    "Runtime acknowledged the desired specification."
                } else {
                    "Runtime reported its current state."
                },
            ),
            Self::Failed => (
                "False",
                "RuntimeFailed",
                if updated {
                    "Runtime could not apply the desired specification."
                } else {
                    "Runtime reported its current state."
                },
            ),
            Self::Pending => (
                "False",
                "Reconciling",
                "Waiting for runtime acknowledgement.",
            ),
            Self::Disconnected => (
                "False",
                if updated {
                    "Reconciling"
                } else {
                    "HostOffline"
                },
                "Waiting for host registration.",
            ),
            Self::Terminating => ("False", "Terminating", "Node is terminating."),
        };
        Condition {
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: Some(generation),
            type_: type_.into(),
            status: status.into(),
            reason: reason.into(),
            message: message.unwrap_or(default_message).into(),
        }
    }
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DispatchStatus {
    /// Artifact selected for custom download or apply. Configuration changes MUST preserve it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_reference: Option<String>,
    pub generation: i64,
    pub operation: crate::registration::Operation,
    pub operation_id: String,
    pub session_id: Option<String>,
    pub success: Option<bool>,
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_adapter: Option<super::adapter::ResolvedAdapter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_reference: Option<String>,
    /// Desired dispatch and its latest runtime acknowledgement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<DispatchStatus>,
    /// Observed stage of the node lifecycle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<NodePhase>,
    /// Human-readable explanation of the current node state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Spec generation reflected in this status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// Latest restart token acknowledged by the runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_restart_token: Option<String>,
    /// Host UID that produced this status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reporting_session: Option<String>,
    /// Node health observations keyed by condition type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "conditions_schema")]
    pub conditions: Vec<Condition>,
}

pub(super) fn reported_metadata_schema(_: &mut SchemaGenerator) -> Schema {
    json!({"type":"object", "x-kubernetes-preserve-unknown-fields":true})
        .try_into()
        .unwrap()
}

fn conditions_schema(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = <Vec<Condition>>::json_schema(generator);
    schema.insert("x-kubernetes-list-type".into(), json!("map"));
    schema.insert("x-kubernetes-list-map-keys".into(), json!(["type"]));
    schema
}

/// Exactly one variant key selects the node type and contains its settings.
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NodeSettings {
    /// Audio capture and encoding configuration.
    AudioSource(AudioSourceSettings),
    /// Speech recognition configuration.
    Transcriber(TranscriberSettings),
    /// Text translation configuration.
    Translator(TranslatorSettings),
    /// Subtitle presentation configuration.
    Display(DisplaySettings),
    /// Arbitrary settings for the referenced extension.
    Custom(super::adapter::PluginSettings),
}

pub fn crds()
-> Vec<k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition>
{
    vec![
        SuntitleHost::crd(),
        SuntitleNode::crd(),
        SuntitleExtension::crd(),
    ]
}

// TODO: replace with serde saphyr
pub fn crd_yaml() -> Result<String, serde_yaml::Error> {
    let mut yaml = String::new();
    for crd in crds() {
        yaml.push_str("---\n");
        yaml.push_str(&serde_yaml::to_string(&crd)?);
    }
    Ok(yaml)
}

macro_rules! openapi_schema {
    ($($ty:ty),+ $(,)?) => {$(
        impl utoipa::PartialSchema for $ty {

            fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
                let mut settings = schemars::generate::SchemaSettings::draft2020_12();
                settings.inline_subschemas = true;
                let schema = settings.clone().into_generator().into_root_schema_for::<Self>();
                let mut value = serde_json::to_value(schema).expect("resource schema serialization");
                if matches!(stringify!($ty), "SuntitleHost" | "SuntitleNode" | "SuntitleExtension") {
                    let metadata = settings.into_generator().into_root_schema_for::<k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta>();
                    value["properties"]["metadata"] = serde_json::to_value(metadata).expect("metadata schema");
                    value["properties"]["apiVersion"] = json!({"type":"string", "enum":["control.suntitle.io/v1alpha1"]});
                    value["properties"]["kind"] = json!({"type":"string", "enum":[stringify!($ty)]});
                    value["required"] = json!(["apiVersion", "kind", "metadata", "spec"]);
                }
                serde_json::from_value(value)
                    .expect("resource OpenAPI schema")
            }
        }
        impl utoipa::ToSchema for $ty {}
    )+};
}

openapi_schema!(
    SuntitleHost,
    SuntitleHostSpec,
    SuntitleNode,
    SuntitleNodeSpec,
    SuntitleExtension,
    SuntitleExtensionSpec,
    ExtensionStatus,
    HostStatus,
    NodeStatus
);

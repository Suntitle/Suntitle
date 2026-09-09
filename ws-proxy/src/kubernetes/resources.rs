//! New resource contracts; the legacy storage and REST models remain in use.
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::{CustomResource, CustomResourceExt, KubeSchema};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::models::{
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
    pub name: String,
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRef {
    /// Name of the referenced resource in the same namespace.
    #[schemars(
        length(min = 1, max = 253),
        regex(pattern = "^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$")
    )]
    pub name: String,
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
pub struct SuntitleNodeSpec {
    /// Display name shown to users.
    #[schemars(length(min = 1, max = 128))]
    pub name: String,
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
    /// Opaque token whose change requests a node restart.
    #[schemars(length(min = 1, max = 128))]
    pub restart_token: Option<String>,
    /// Configuration for exactly one node type. The selected type MUST remain unchanged.
    #[x_kube(validation = Rule::new(concat!(
        "has(self.audio_source) == has(oldSelf.audio_source) && ",
        "has(self.transcriber) == has(oldSelf.transcriber) && ",
        "has(self.translator) == has(oldSelf.translator) && ",
        "has(self.display) == has(oldSelf.display)",
    )).message("node type is immutable"))]
    pub settings: NodeSettings,
}

fn enabled_default() -> bool {
    true
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
    /// Current agent session identifier used to reject stale reports.
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

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
pub enum NodePhase {
    Pending,
    Running,
    Disabled,
    Failed,
    Disconnected,
    Terminating,
}

#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
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
    /// Host agent session that produced this status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reporting_session: Option<String>,
    /// Node health observations keyed by condition type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "conditions_schema")]
    pub conditions: Vec<Condition>,
}

fn reported_metadata_schema(_: &mut SchemaGenerator) -> Schema {
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
#[derive(JsonSchema, Serialize, Deserialize, Clone, Debug)]
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
}

pub fn crds()
-> Vec<k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition>
{
    vec![SuntitleHost::crd(), SuntitleNode::crd()]
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

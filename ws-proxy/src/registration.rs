use crate::kubernetes::{
    adapter::ResolvedAdapter,
    resources::{ResourceIdentity, SuntitleNodeSpec},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyIdentity {
    pub target: ResourceIdentity,
    pub host_uid: String,
    pub session_id: String,
    pub generation: i64,
    pub restart_token: Option<String>,
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope<T> {
    pub protocol_version: u8,
    #[serde(flatten)]
    pub message: T,
}
impl<T> Envelope<T> {
    pub fn new(message: T) -> Self {
        Self {
            protocol_version: 1,
            message,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RegistrationClientMessage {
    Register {
        host_name: String,
        agent_version: String,
        reported_metadata: BTreeMap<String, Value>,
        ip: Option<String>,
    },
    Heartbeat {
        host_uid: String,
        session_id: String,
    },
    Acknowledgement {
        #[serde(flatten)]
        identity: ApplyIdentity,
        operation: Operation,
        success: bool,
        message: Option<String>,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Apply,
    Download,
    Remove,
}

/// Registration replies and node commands for the current host session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RegistrationServerMessage {
    Registered {
        host: ResourceIdentity,
        session_id: String,
    },
    Apply {
        #[serde(flatten)]
        identity: ApplyIdentity,
        spec: Box<SuntitleNodeSpec>,
    },
    /// Download and prepare a custom extension without starting its process.
    DownloadArtifact {
        /// Node revision, dispatch operation, and host session selecting this installation.
        #[serde(flatten)]
        identity: ApplyIdentity,
        /// Published extension payload selected by the controller.
        reference: String,
        /// Adapter with defaults and node-specific settings already resolved.
        adapter: Box<ResolvedAdapter>,
    },
    Remove {
        #[serde(flatten)]
        identity: ApplyIdentity,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_acknowledgement_round_trip_preserves_complete_identity() {
        let identity = ApplyIdentity {
            target: ResourceIdentity {
                namespace: "suntitle".into(),
                name: "node".into(),
                uid: "uid".into(),
            },
            host_uid: "host".into(),
            session_id: "session".into(),
            generation: 4,
            restart_token: Some("token".into()),
            operation_id: "operation".into(),
        };
        let message = Envelope::new(RegistrationClientMessage::Acknowledgement {
            identity: identity.clone(),
            operation: Operation::Apply,
            success: true,
            message: None,
        });
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["protocolVersion"], 1);
        assert_eq!(value["sessionId"], "session");
        let decoded: Envelope<RegistrationClientMessage> = serde_json::from_value(value).unwrap();
        let RegistrationClientMessage::Acknowledgement {
            identity: actual, ..
        } = decoded.message
        else {
            panic!("wrong message")
        };
        assert_eq!(actual, identity);
    }
}

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::models::{Host, NodeUpdate};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRuntimeStatus {
    Starting,
    Running,
    Stopped,
    Failed,
}

/// First message sent by a Diamant agent after opening `/register`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegistrationClientMessage {
    RegisterHost {
        id: Uuid,
        name: String,
        ip: IpAddr,
        version: String,
        metadata: Option<BTreeMap<String, Value>>,
    },
    HealthOk {
        host_id: Uuid,
        ip: IpAddr,
        metadata: Option<BTreeMap<String, Value>>,
    },
    NodeStatus {
        host_id: Uuid,
        node_id: Uuid,
        revision: u64,
        status: NodeRuntimeStatus,
        message: Option<String>,
    },
}

/// Registration response returned by VS Proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegistrationServerMessage {
    Registered {
        host: Host,
    },
    ApplyNode {
        node_id: Uuid,
        revision: u64,
        update: NodeUpdate,
    },
    RestartNode {
        node_id: Uuid,
        revision: u64,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_message_uses_a_versioned_host_identity() {
        let json = serde_json::to_string(&RegistrationClientMessage::RegisterHost {
            id: Uuid::parse_str("20000000-0000-4000-8000-000000000001").unwrap(),
            name: "studio-host".to_owned(),
            ip: "192.0.2.10".parse().unwrap(),
            version: "0.1.0".to_owned(),
            metadata: None,
        })
        .expect("registration serializes");

        assert_eq!(
            json,
            r#"{"type":"register_host","id":"20000000-0000-4000-8000-000000000001","name":"studio-host","ip":"192.0.2.10","version":"0.1.0","metadata":null}"#
        );
    }

    #[test]
    fn health_message_identifies_the_registered_host() {
        let host_id = Uuid::parse_str("20000000-0000-4000-8000-000000000001").unwrap();
        let json = serde_json::to_string(&RegistrationClientMessage::HealthOk {
            host_id,
            ip: "192.0.2.11".parse().unwrap(),
            metadata: Some(BTreeMap::from([(
                "arch".to_owned(),
                Value::String("aarch64".to_owned()),
            )])),
        })
        .expect("health serializes");

        assert_eq!(
            json,
            r#"{"type":"health_ok","host_id":"20000000-0000-4000-8000-000000000001","ip":"192.0.2.11","metadata":{"arch":"aarch64"}}"#
        );
    }

    #[test]
    fn lifecycle_status_round_trips() {
        let value = RegistrationClientMessage::NodeStatus {
            host_id: Uuid::nil(),
            node_id: Uuid::new_v4(),
            revision: 7,
            status: NodeRuntimeStatus::Running,
            message: None,
        };
        let json = serde_json::to_string(&value).unwrap();
        let decoded: RegistrationClientMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            RegistrationClientMessage::NodeStatus {
                revision: 7,
                status: NodeRuntimeStatus::Running,
                ..
            }
        ));
    }

    #[test]
    fn apply_node_round_trips() {
        let update: NodeUpdate = serde_json::from_value(serde_json::json!({
            "type":"display", "name":"screen", "description":null, "enabled":true,
            "settings":{"maxLines":3,"fontScale":1.0,"showOriginal":false,"theme":"dark"},
            "inputNodeId":null
        }))
        .unwrap();
        let value = RegistrationServerMessage::ApplyNode {
            node_id: Uuid::new_v4(),
            revision: 3,
            update,
        };
        let json = serde_json::to_string(&value).unwrap();
        let decoded = serde_json::from_str::<RegistrationServerMessage>(&json);
        assert!(decoded.is_ok(), "{json}: {decoded:?}");
    }
}

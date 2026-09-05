use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::models::Host;

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
}

/// Registration response returned by VS Proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegistrationServerMessage {
    Registered { host: Host },
    Error { message: String },
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
}

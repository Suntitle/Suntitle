use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    AudioSourceNode, DisplayNode, Host, NodeBase, NodeSettings, NodeStatus, NodeType, NodeUpdate,
    TranscriberNode, TranslatorNode,
};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    AudioSource(AudioSourceNode),
    Transcriber(TranscriberNode),
    Translator(TranslatorNode),
    Display(DisplayNode),
}

impl Node {
    pub fn base(&self) -> &NodeBase {
        match self {
            Self::AudioSource(node) => &node.base,
            Self::Transcriber(node) => &node.base,
            Self::Translator(node) => &node.base,
            Self::Display(node) => &node.base,
        }
    }

    pub fn base_mut(&mut self) -> &mut NodeBase {
        match self {
            Self::AudioSource(node) => &mut node.base,
            Self::Transcriber(node) => &mut node.base,
            Self::Translator(node) => &mut node.base,
            Self::Display(node) => &mut node.base,
        }
    }

    pub fn id(&self) -> Uuid {
        self.base().id
    }

    pub fn node_type(&self) -> NodeType {
        match self {
            Self::AudioSource(_) => NodeType::AudioSource,
            Self::Transcriber(_) => NodeType::Transcriber,
            Self::Translator(_) => NodeType::Translator,
            Self::Display(_) => NodeType::Display,
        }
    }

    pub fn status(&self) -> NodeStatus {
        self.base().status
    }

    pub fn enabled(&self) -> bool {
        self.base().enabled
    }

    pub fn set_status(&mut self, status: NodeStatus) {
        let base = self.base_mut();
        base.status = status;
        base.updated_at = Utc::now();
    }

    pub fn from_update(id: Uuid, host: Host, update: &NodeUpdate) -> Self {
        let now = Utc::now();
        let update_base = update.base();
        let base = NodeBase {
            id,
            name: update_base.name.clone(),
            description: update_base.description.clone(),
            enabled: update_base.enabled,
            status: NodeStatus::Disconnected,
            host,
            connections: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        match &update.settings {
            NodeSettings::AudioSource(settings) => Self::AudioSource(AudioSourceNode {
                base,
                settings: settings.clone(),
            }),
            NodeSettings::Transcriber(settings) => Self::Transcriber(TranscriberNode {
                base,
                settings: settings.clone(),
            }),
            NodeSettings::Translator(settings) => Self::Translator(TranslatorNode {
                base,
                settings: settings.clone(),
            }),
            NodeSettings::Display(settings) => Self::Display(DisplayNode {
                base,
                settings: settings.clone(),
            }),
        }
    }

    pub fn with_update(&self, update: &NodeUpdate) -> Self {
        let old = self.base();
        let mut node = Self::from_update(old.id, old.host.clone(), update);
        node.base_mut().connections = old.connections.clone();
        node.base_mut().created_at = old.created_at;
        node
    }
}

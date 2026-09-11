use crate::{kubernetes::resources::ResourceIdentity, registration::RegistrationServerMessage};
use std::collections::HashMap;
use tokio::sync::{mpsc, watch};

const QUEUE_CAPACITY: usize = 32;

pub struct Connection {
    pub host: ResourceIdentity,
    pub session_id: String,
    pub outbound: mpsc::Receiver<RegistrationServerMessage>,
    pub closed: watch::Receiver<bool>,
}

struct Route {
    host: ResourceIdentity,
    outbound: mpsc::Sender<RegistrationServerMessage>,
    closed: watch::Sender<bool>,
    operations: HashMap<String, String>,
}

#[derive(Default)]
pub(crate) struct Connections {
    routes: HashMap<String, Route>,
}

impl Connections {
    pub fn open(&mut self, host: ResourceIdentity) -> Connection {
        let session_id = host.uid.clone();
        self.close(&host.uid);
        let (outbound, receiver) = mpsc::channel(QUEUE_CAPACITY);
        let (closed, close_rx) = watch::channel(false);
        self.routes.insert(
            host.uid.clone(),
            Route {
                host: host.clone(),
                outbound,
                closed,
                operations: HashMap::new(),
            },
        );
        Connection {
            host,
            session_id,
            outbound: receiver,
            closed: close_rx,
        }
    }

    pub fn contains(&self, uid: &str, session: &str) -> bool {
        uid == session && self.routes.contains_key(uid)
    }

    pub fn host(&self, uid: &str) -> Option<ResourceIdentity> {
        self.routes.get(uid).map(|route| route.host.clone())
    }

    pub fn hosts(&self) -> Vec<ResourceIdentity> {
        self.routes
            .values()
            .map(|route| route.host.clone())
            .collect()
    }

    pub fn close(&mut self, uid: &str) -> Option<ResourceIdentity> {
        let route = self.routes.remove(uid)?;
        let _ = route.closed.send(true);
        Some(route.host)
    }

    pub fn dispatched(&self, uid: &str, node: &str, operation: &str) -> bool {
        self.routes
            .get(uid)
            .is_some_and(|route| route.operations.get(node).is_some_and(|sent| sent == operation))
    }

    pub fn mark_dispatched(&mut self, uid: &str, node: String, operation: String) {
        if let Some(route) = self.routes.get_mut(uid) {
            route.operations.insert(node, operation);
        }
    }

    pub fn send(&self, uid: &str, session: &str, message: RegistrationServerMessage) -> bool {
        self.routes
            .get(uid)
            .filter(|_| uid == session)
            .is_some_and(|route| route.outbound.try_send(message).is_ok())
    }
}

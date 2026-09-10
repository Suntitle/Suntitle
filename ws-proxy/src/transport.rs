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
}

#[derive(Default)]
pub(crate) struct Connections {
    routes: HashMap<(String, String), Route>,
}

impl Connections {
    pub fn open(&mut self, host: ResourceIdentity, session_id: String) -> Connection {
        self.close(&host.uid);
        let (outbound, receiver) = mpsc::channel(QUEUE_CAPACITY);
        let (closed, close_rx) = watch::channel(false);
        self.routes.insert(
            (host.uid.clone(), session_id.clone()),
            Route {
                host: host.clone(),
                outbound,
                closed,
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
        self.routes.contains_key(&(uid.into(), session.into()))
    }

    pub fn host(&self, uid: &str) -> Option<ResourceIdentity> {
        self.routes
            .values()
            .find(|route| route.host.uid == uid)
            .map(|route| route.host.clone())
    }

    pub fn hosts(&self) -> Vec<ResourceIdentity> {
        self.routes
            .values()
            .map(|route| route.host.clone())
            .collect()
    }

    pub fn close(&mut self, uid: &str) -> Option<ResourceIdentity> {
        let key = self.routes.keys().find(|(host, _)| host == uid)?.clone();
        let route = self.routes.remove(&key)?;
        let _ = route.closed.send(true);
        Some(route.host)
    }

    pub fn send(&self, uid: &str, session: &str, message: RegistrationServerMessage) -> bool {
        self.routes
            .get(&(uid.into(), session.into()))
            .is_some_and(|route| route.outbound.try_send(message).is_ok())
    }
}

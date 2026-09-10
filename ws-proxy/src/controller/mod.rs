//! Controller lifecycle and shared reconciliation context.
mod conditions;
mod dispatch;
mod host;
mod runtime;
mod session;
#[cfg(test)]
mod tests;

use crate::{kubernetes::Error, registration::RegistrationClientMessage, transport::Connections};
use kube::{
    Api, Client,
    api::ListParams,
    runtime::{controller::Action, finalizer, reflector},
};
use session::State;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Mutex, Notify},
    task::JoinHandle,
};

pub use crate::transport::Connection;

#[derive(Clone)]
pub struct Controller {
    context: Arc<Context>,
}
struct Context {
    state: Mutex<State>,
    stopping: AtomicBool,
    stopped: Notify,
}

#[derive(Debug)]
enum ReconcileError {
    Failed(Error),
    CleanupPending,
}
impl std::fmt::Display for ReconcileError {

    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(error) => error.fmt(f),
            Self::CleanupPending => f.write_str("cleanup is pending"),
        }
    }
}
impl std::error::Error for ReconcileError {}

fn error_policy<K>(_: Arc<K>, error: &ReconcileError, _: Arc<Context>) -> Action {
    if !matches!(error, ReconcileError::CleanupPending) {
        eprintln!("reconcile: {error}");
    }
    Action::requeue(Duration::from_secs(5))
}

fn finalizer_error(error: finalizer::Error<ReconcileError>) -> ReconcileError {
    match error {
        finalizer::Error::CleanupFailed(error) | finalizer::Error::ApplyFailed(error) => error,
        error => ReconcileError::Failed(error.into()),
    }
}

impl Controller {

    /// Spec changes and runtime observations use separate controllers.
    pub fn start(client: Client, namespace: &str) -> (Self, JoinHandle<()>) {
        let hosts_api = Api::namespaced(client.clone(), namespace);
        let nodes_api = Api::namespaced(client, namespace);
        let context = Arc::new(Context {
            stopping: AtomicBool::new(false),
            stopped: Notify::new(),
            state: Mutex::new(State {
                hosts_api: hosts_api.clone(),
                nodes_api: nodes_api.clone(),
                connections: Connections::default(),
            }),
        });
        let (nodes, writer) = reflector::store();
        let run_context = context.clone();
        let task = tokio::spawn(async move {
            tokio::select! {
                _ = host::run(hosts_api.clone(), nodes_api.clone(), run_context.clone()) => {},
                _ = dispatch::run(nodes_api.clone(), nodes.clone(), writer, run_context.clone()) => {},
                _ = runtime::run(nodes_api.clone(), hosts_api.clone(), nodes, run_context.clone()) => {},
                _ = run_context.stopped.notified() => {},
            }
            let mut state = run_context.state.lock().await;
            for host in state.connections.hosts() {
                state.close(&host.uid);
            }
            let hosts = match state.hosts_api.list(&ListParams::default()).await {
                Ok(hosts) => hosts,
                Err(error) => {
                    eprintln!("host shutdown list: {error}");
                    return;
                }
            };
            for host in hosts {
                if let Err(error) = state.offline(&host).await {
                    eprintln!("host shutdown: {error}");
                }
            }
        });
        (Self { context }, task)
    }

    pub async fn register(&self, message: RegistrationClientMessage) -> Result<Connection, String> {
        let RegistrationClientMessage::Register {
            host_name,
            agent_version,
            reported_metadata,
            ip,
        } = message
        else {
            return Err("registration required".into());
        };
        let mut state = self.context.state.lock().await;
        if self.context.stopping.load(Ordering::Relaxed) {
            return Err("controller is stopping".into());
        }
        state
            .register(host_name, agent_version, reported_metadata, ip)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn report(&self, host: &str, session: &str, message: RegistrationClientMessage) {
        if let Err(error) = self
            .context
            .state
            .lock()
            .await
            .report(host, session, message)
            .await
        {
            eprintln!("runtime report: {error}");
        }
    }

    pub async fn disconnect(&self, host: &str, session: &str) {
        let mut state = self.context.state.lock().await;
        if let Err(error) = state.disconnect(host, session).await {
            eprintln!("disconnect: {error}");
        }
    }

    pub async fn shutdown(&self) {
        self.context.stopping.store(true, Ordering::Relaxed);
        self.context.stopped.notify_one();
    }
}

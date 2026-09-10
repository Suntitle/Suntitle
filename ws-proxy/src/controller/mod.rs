//! Embedded kube-runtime controllers and the runtime session registry.
mod conditions;
use crate::{
    kubernetes::{Error, resources::*},
    registration::*,
    transport::Connections,
};
use chrono::Utc;
use futures_util::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::{
    Api, Client, Resource, ResourceExt,
    api::{ListParams, Patch, PatchParams, PostParams},
    runtime::{
        WatchStreamExt,
        controller::{Action, Config, Controller as KubeController},
        reflector,
        reflector::ObjectRef,
        watcher,
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Mutex, mpsc, watch},
    task::JoinHandle,
    time::Instant,
};

pub use crate::transport::Connection;
const EXPIRY: Duration = Duration::from_secs(45);
#[derive(Clone)]
pub struct Controller {
    context: Arc<Context>,
    stop: watch::Sender<bool>,
    ready: watch::Receiver<bool>,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum Key {
    Host(String, String),
    Node(String, String),
}
struct Context {
    state: Mutex<State>,
    ready: watch::Sender<bool>,
}

struct Triggers {
    namespace: String,
    hosts: mpsc::UnboundedSender<ObjectRef<SuntitleHost>>,
    nodes: mpsc::UnboundedSender<ObjectRef<SuntitleNode>>,
}
impl Triggers {
    fn insert(&self, key: Key) {
        match key {
            Key::Host(name, _) => {
                let _ = self
                    .hosts
                    .send(ObjectRef::new(&name).within(&self.namespace));
            }
            Key::Node(name, _) => {
                let _ = self
                    .nodes
                    .send(ObjectRef::new(&name).within(&self.namespace));
            }
        }
    }
    fn extend(&self, keys: impl IntoIterator<Item = Key>) {
        for key in keys {
            self.insert(key);
        }
    }
}

#[derive(Debug)]
struct ReconcileError(Error);
impl std::fmt::Display for ReconcileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for ReconcileError {}

fn error_policy<K>(_: Arc<K>, error: &ReconcileError, _: Arc<Context>) -> Action {
    eprintln!("reconcile: {error}");
    Action::requeue(Duration::from_secs(5))
}

async fn reconcile_host(
    host: Arc<SuntitleHost>,
    ctx: Arc<Context>,
) -> Result<Action, ReconcileError> {
    let mut state = ctx.state.lock().await;
    if !state.synced() {
        return Ok(Action::requeue(Duration::from_secs(1)));
    }
    state.reconcile_host(&host).await.map_err(ReconcileError)?;
    Ok(Action::await_change())
}

async fn reconcile_node(
    node: Arc<SuntitleNode>,
    ctx: Arc<Context>,
) -> Result<Action, ReconcileError> {
    let mut state = ctx.state.lock().await;
    if !state.synced() {
        return Ok(Action::requeue(Duration::from_secs(1)));
    }
    state.reconcile_node(&node).await.map_err(ReconcileError)?;
    Ok(Action::await_change())
}

fn predicate<K: Resource>(resource: &K) -> Option<u64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    relevant(resource.meta()).to_string().hash(&mut hasher);
    Some(hasher.finish())
}

struct State {
    hosts_api: Api<SuntitleHost>,
    nodes_api: Api<SuntitleNode>,
    resync: bool,
    hosts_synced: bool,
    nodes_synced: bool,
    triggers: Triggers,
    connections: Connections,
}
impl Controller {
    pub fn start(client: Client, namespace: &str) -> (Self, JoinHandle<()>) {
        let (ready_tx, ready) = watch::channel(false);
        let (stop, mut stopped) = watch::channel(false);
        let hosts_api = Api::namespaced(client.clone(), namespace);
        let nodes_api = Api::namespaced(client, namespace);
        let (host_tx, host_rx) = mpsc::unbounded_channel();
        let (node_tx, node_rx) = mpsc::unbounded_channel();
        let context = Arc::new(Context {
            ready: ready_tx,
            state: Mutex::new(State {
                hosts_api: hosts_api.clone(),
                nodes_api: nodes_api.clone(),

                resync: false,
                hosts_synced: false,
                nodes_synced: false,
                triggers: Triggers {
                    namespace: namespace.into(),
                    hosts: host_tx,
                    nodes: node_tx,
                },
                connections: Connections::default(),
            }),
        });
        let (hosts, host_writer) = reflector::store();
        let (nodes, node_writer) = reflector::store();
        let host_context = context.clone();
        let host_changes = context.clone();
        let host_stream = watcher(hosts_api, watcher::Config::default())
            .default_backoff()
            .reflect(host_writer)
            .then(move |event| {
                let ctx = host_context.clone();
                async move {
                    if let Ok(event) = &event {
                        let mut state = ctx.state.lock().await;
                        if let Err(error) = state.host_event(event).await {
                            state.resync = true;
                            eprintln!("host lifecycle: {error}");
                        }
                        ctx.ready.send_replace(state.synced());
                    }
                    event
                }
            })
            .applied_objects()
            .predicate_filter(predicate, Default::default())
            .then(move |object| {
                let ctx = host_changes.clone();
                async move {
                    if let Ok(host) = &object {
                        let mut state = ctx.state.lock().await;
                        if let Err(error) = state
                            .schedule_host_nodes(&host.name_any(), &host.uid().unwrap_or_default())
                            .await
                        {
                            state.resync = true;
                            eprintln!("host dependents: {error}");
                        }
                    }
                    object
                }
            });
        let node_context = context.clone();
        let node_changes = context.clone();
        let node_stream = watcher(nodes_api, watcher::Config::default())
            .default_backoff()
            .reflect(node_writer)
            .then(move |event| {
                let ctx = node_context.clone();
                async move {
                    if let Ok(event) = &event {
                        let mut state = ctx.state.lock().await;
                        if let Err(error) = state.node_event(event).await {
                            state.resync = true;
                            eprintln!("node lifecycle: {error}");
                        }
                        ctx.ready.send_replace(state.synced());
                    }
                    event
                }
            })
            .applied_objects()
            .predicate_filter(predicate, Default::default())
            .then(move |object| {
                let ctx = node_changes.clone();
                async move {
                    if let Ok(node) = &object {
                        let mut state = ctx.state.lock().await;
                        if let Err(error) = state.dependents(&node.name_any()).await {
                            state.resync = true;
                            eprintln!("input dependents: {error}");
                        }
                    }
                    object
                }
            });
        let run_context = context.clone();
        let task = tokio::spawn(async move {
            let host_controller = KubeController::for_stream(host_stream, hosts)
                .with_config(Config::default().concurrency(1))
                .reconcile_on(tokio_stream::wrappers::UnboundedReceiverStream::new(
                    host_rx,
                ))
                .run(reconcile_host, error_policy, run_context.clone())
                .for_each(|result| async move {
                    if let Err(error) = result {
                        eprintln!("host controller: {error}");
                    }
                });
            let node_controller = KubeController::for_stream(node_stream, nodes)
                .with_config(Config::default().concurrency(1))
                .reconcile_on(tokio_stream::wrappers::UnboundedReceiverStream::new(
                    node_rx,
                ))
                .run(reconcile_node, error_policy, run_context.clone())
                .for_each(|result| async move {
                    if let Err(error) = result {
                        eprintln!("node controller: {error}");
                    }
                });
            let expiry = async {
                let mut timer = tokio::time::interval(Duration::from_secs(1));
                loop {
                    timer.tick().await;
                    if let Err(error) = run_context.state.lock().await.tick().await {
                        eprintln!("heartbeat expiry: {error}");
                    }
                }
            };
            tokio::select! {
                _ = host_controller => {},
                _ = node_controller => {},
                _ = expiry => {},
                _ = stopped.changed() => {},
            }
            run_context.ready.send_replace(false);
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
                if let Err(error) = state.reconcile_host(&host).await {
                    eprintln!("host shutdown: {error}");
                }
            }
        });
        (
            Self {
                context,
                ready,
                stop,
            },
            task,
        )
    }
    pub async fn synchronized(&mut self) -> Result<(), Error> {
        self.ready.wait_for(|r| *r).await?;
        Ok(())
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
        if *self.stop.borrow() || !state.synced() {
            return Err("controller is not ready".into());
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
        self.stop.send_replace(true);
    }
}
impl State {
    fn synced(&self) -> bool {
        self.hosts_synced && self.nodes_synced
    }
    async fn schedule_host_nodes(&self, name: &str, uid: &str) -> Result<(), Error> {
        for node in self.nodes_api.list(&ListParams::default()).await? {
            if node.spec.host_ref.name == name
                || node
                    .owner_references()
                    .iter()
                    .any(|owner| owner.controller == Some(true) && owner.uid == uid)
            {
                self.triggers
                    .insert(Key::Node(node.name_any(), node.uid().unwrap_or_default()));
            }
        }
        Ok(())
    }
    async fn all(&mut self) -> Result<(), Error> {
        let hosts = self.hosts_api.list(&ListParams::default()).await?;
        let nodes = self.nodes_api.list(&ListParams::default()).await?;
        for connection in self.connections.hosts() {
            if !hosts.iter().any(|host| {
                host.name_any() == connection.name
                    && host.uid().as_deref() == Some(&connection.uid)
                    && host.metadata.deletion_timestamp.is_none()
            }) {
                self.close(&connection.uid);
            }
        }
        self.triggers.extend(
            hosts
                .iter()
                .map(|host| Key::Host(host.name_any(), host.uid().unwrap_or_default())),
        );
        self.triggers.extend(
            nodes
                .iter()
                .map(|node| Key::Node(node.name_any(), node.uid().unwrap_or_default())),
        );
        self.resync = false;
        Ok(())
    }
    async fn host_event(&mut self, event: &watcher::Event<SuntitleHost>) -> Result<(), Error> {
        match event {
            watcher::Event::Init => self.hosts_synced = false,
            watcher::Event::InitDone => {
                self.hosts_synced = true;
                self.resync = true;
            }
            watcher::Event::Delete(host) => {
                self.close(&host.uid().unwrap_or_default());
                self.schedule_host_nodes(&host.name_any(), &host.uid().unwrap_or_default())
                    .await?;
            }
            watcher::Event::Apply(host) => {
                for connection in self.connections.hosts() {
                    if connection.name == host.name_any()
                        && (host.uid().as_deref() != Some(&connection.uid)
                            || host.metadata.deletion_timestamp.is_some())
                    {
                        self.close(&connection.uid);
                    }
                }
            }
            watcher::Event::InitApply(_) => {}
        }
        Ok(())
    }
    async fn node_event(&mut self, event: &watcher::Event<SuntitleNode>) -> Result<(), Error> {
        match event {
            watcher::Event::Init => self.nodes_synced = false,
            watcher::Event::InitDone => {
                self.nodes_synced = true;
                self.resync = true;
            }
            watcher::Event::Delete(node) => {
                self.dependents(&node.name_any()).await?;
            }
            watcher::Event::Apply(_) | watcher::Event::InitApply(_) => {}
        }
        Ok(())
    }
    async fn dependents(&self, name: &str) -> Result<(), Error> {
        for node in self.nodes_api.list(&ListParams::default()).await? {
            if node
                .spec
                .input_ref
                .as_ref()
                .is_some_and(|input| input.name == name)
            {
                self.triggers
                    .insert(Key::Node(node.name_any(), node.uid().unwrap_or_default()));
            }
        }
        Ok(())
    }
    fn close(&mut self, uid: &str) {
        if let Some(host) = self.connections.close(uid) {
            self.triggers.insert(Key::Host(host.name, uid.into()));
            self.resync = true;
        }
    }
    async fn disconnect(&mut self, uid: &str, session: &str) -> Result<(), Error> {
        let Some(identity) = self.connections.host(uid) else {
            return Ok(());
        };
        if !self.connections.contains(uid, session) {
            return Ok(());
        }
        let host = self.hosts_api.get_opt(&identity.name).await;
        self.close(uid);
        if let Some(host) = host?.filter(|host| current_session(host, uid, session)) {
            self.reconcile_host(&host).await?;
        }
        Ok(())
    }
    async fn tick(&mut self) -> Result<(), Error> {
        if !self.synced() {
            return Ok(());
        }
        if self.resync {
            self.all().await?;
        }
        for host in self.hosts_api.list(&ListParams::default()).await? {
            let uid = host.uid().ok_or("host UID missing")?;
            let online = matches!(
                host.status
                    .as_ref()
                    .and_then(|status| status.presence.as_ref()),
                Some(Presence::Online)
            );
            if (online || self.connections.host(&uid).is_some())
                && self.active_session(&host).is_none()
            {
                self.triggers
                    .insert(Key::Host(host.name_any(), uid.clone()));
                self.schedule_host_nodes(&host.name_any(), &uid).await?;
            }
        }
        Ok(())
    }
    fn active_session(&self, host: &SuntitleHost) -> Option<String> {
        if host.metadata.deletion_timestamp.is_some() || heartbeat_expired(host, Utc::now()) {
            return None;
        }
        let session = host.status.as_ref()?.session_id.as_ref()?;
        self.connections
            .contains(host.metadata.uid.as_deref()?, session)
            .then(|| session.clone())
    }
    async fn register(
        &mut self,
        name: String,
        version: String,
        metadata: BTreeMap<String, Value>,
        ip: Option<String>,
    ) -> Result<Connection, Error> {
        if name.is_empty() || version.trim().is_empty() {
            return Err("host name and agent version are required".into());
        }
        let mut host = match self.hosts_api.get_opt(&name).await? {
            Some(host) => host,
            None => {
                self.hosts_api
                    .create(
                        &PostParams::default(),
                        &SuntitleHost::new(&name, SuntitleHostSpec { name: name.clone() }),
                    )
                    .await?
            }
        };
        if host.metadata.deletion_timestamp.is_some() {
            return Err("host is terminating".into());
        }
        let identity = identity(&host.metadata)?;
        self.close(&identity.uid);
        let id = uuid::Uuid::new_v4().to_string();
        let previous = host.status.clone();
        let status = host.status.get_or_insert_default();
        status.ip = ip;
        status.agent_version = Some(version);
        status.reported_metadata = metadata;
        status.session_id = Some(id.clone());
        status.last_heartbeat = Some(Utc::now());
        status.presence = Some(Presence::Online);
        self.host_status(&mut host, &previous).await?;
        let connection = self.connections.open(identity.clone(), id);
        self.triggers
            .insert(Key::Host(name.clone(), identity.uid.clone()));
        self.resync = true;
        Ok(connection)
    }
    async fn host_status(
        &self,
        host: &mut SuntitleHost,
        previous: &Option<HostStatus>,
    ) -> Result<(), Error> {
        if let Some(status) = host.status.as_mut() {
            conditions::merge(
                previous
                    .as_ref()
                    .map(|status| status.conditions.as_slice())
                    .unwrap_or_default(),
                &mut status.conditions,
            );
        }
        if json!(previous) != json!(host.status) {
            *host = self.hosts_api.patch_status(&host.name_any(), &PatchParams::default(), &Patch::Merge(json!({"metadata":{"resourceVersion":host.resource_version(),"uid":host.uid()},"status":host.status}))).await?;
        }
        Ok(())
    }
    async fn node_status(
        &self,
        node: &mut SuntitleNode,
        previous: &Option<NodeStatus>,
    ) -> Result<(), Error> {
        if let Some(status) = node.status.as_mut() {
            conditions::merge(
                previous
                    .as_ref()
                    .map(|status| status.conditions.as_slice())
                    .unwrap_or_default(),
                &mut status.conditions,
            );
        }
        if json!(previous) != json!(node.status) {
            // Nulls MUST clear optional fields omitted by status serialization.
            let mut value = json!(node.status);
            for field in ["message", "reportingSession", "acknowledgedRestartToken"] {
                if value.get(field).is_none() {
                    value[field] = Value::Null;
                }
            }
            *node = self.nodes_api.patch_status(&node.name_any(), &PatchParams::default(), &Patch::Merge(json!({"metadata":{"resourceVersion":node.resource_version(),"uid":node.uid()},"status":value}))).await?;
        }
        Ok(())
    }
    async fn reconcile_host(&mut self, host: &SuntitleHost) -> Result<(), Error> {
        let name = host.name_any();
        let uid = host.uid().ok_or("host UID missing")?;
        let mut host = host.clone();
        if host.metadata.deletion_timestamp.is_some() {
            self.close(&uid);
            return Ok(());
        }
        if !host.finalizers().iter().any(|f| f == HOST_FINALIZER) {
            let mut finalizers = host.finalizers().to_vec();
            finalizers.push(HOST_FINALIZER.into());
            host = self.hosts_api.patch(&name, &PatchParams::default(), &Patch::Merge(json!({"metadata":{"resourceVersion":host.resource_version(),"finalizers":finalizers}}))).await?;
            if host.metadata.deletion_timestamp.is_some() {
                return Ok(());
            }
        }
        let generation = host.metadata.generation.unwrap_or(1);
        let previous = host.status.clone();
        let status = host.status.get_or_insert_default();
        let updated = status.conditions.iter().any(|c| {
            c.type_ == "Updated" && c.status == "True" && c.observed_generation == Some(generation)
        });
        if !updated {
            status.conditions.push(Condition {
                last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
                observed_generation: Some(generation),
                type_: "Updated".into(),
                status: "False".into(),
                reason: "Reconciling".into(),
                message: "Applying host configuration.".into(),
            });
            status.conditions.push(Condition {
                last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
                observed_generation: Some(generation),
                type_: "Updated".into(),
                status: "True".into(),
                reason: "DesiredStateApplied".into(),
                message: "Host configuration accepted.".into(),
            });
        }
        let presence = if self.active_session(&host).is_some() {
            Presence::Online
        } else {
            self.close(&uid);
            Presence::Offline
        };
        let status = host.status.get_or_insert_default();
        status.conditions.push(presence.ready_condition(generation));
        status.presence = Some(presence);
        self.host_status(&mut host, &previous).await
    }
    async fn invalid(
        &mut self,
        node: &mut SuntitleNode,
        reason: &str,
        message: &str,
    ) -> Result<(), Error> {
        let previous = node.status.clone();
        let status = node.status.get_or_insert_default();
        status.phase = Some(NodePhase::Failed);
        status.message = Some(message.into());
        status.conditions.push(Condition {
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: Some(node.metadata.generation.unwrap_or(1)),
            type_: "Updated".into(),
            status: "False".into(),
            reason: (reason).into(),
            message: (message).into(),
        });
        status.conditions.push(Condition {
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: Some(node.metadata.generation.unwrap_or(1)),
            type_: "Ready".into(),
            status: "False".into(),
            reason: (reason).into(),
            message: (message).into(),
        });
        self.node_status(node, &previous).await
    }
    async fn reconcile_node(&mut self, node: &SuntitleNode) -> Result<(), Error> {
        let name = node.name_any();
        let mut node = node.clone();
        if node.metadata.deletion_timestamp.is_some() {
            return Ok(());
        }
        let Some(host) = self.hosts_api.get_opt(&node.spec.host_ref.name).await? else {
            return self
                .invalid(
                    &mut node,
                    "HostUnavailable",
                    "Referenced host does not exist.",
                )
                .await;
        };
        let host_uid = host.uid().ok_or("host UID missing")?;
        let owner = host
            .controller_owner_ref(&())
            .ok_or("host owner identity missing")?;
        let controllers: Vec<_> = node
            .owner_references()
            .iter()
            .filter(|o| o.controller == Some(true))
            .collect();
        if controllers.len() > 1
            || controllers.first().is_some_and(|o| {
                o.uid != owner.uid
                    || o.name != owner.name
                    || o.kind != owner.kind
                    || o.api_version != owner.api_version
            })
        {
            return self
                .invalid(
                    &mut node,
                    "InvalidOwner",
                    "Controller owner MUST match the referenced host incarnation.",
                )
                .await;
        }
        self.reconcile_host(&host).await?;
        let mut owners = node.owner_references().to_vec();
        if controllers.is_empty() {
            owners.push(owner);
        }
        let mut finalizers = node.finalizers().to_vec();
        if !finalizers.iter().any(|f| f == NODE_FINALIZER) {
            finalizers.push(NODE_FINALIZER.into());
        }
        if owners != node.owner_references() || finalizers != node.finalizers() {
            node = self.nodes_api.patch(&name, &PatchParams::default(), &Patch::Merge(json!({"metadata":{"resourceVersion":node.resource_version(),"ownerReferences":owners,"finalizers":finalizers}}))).await?;
            if node.metadata.deletion_timestamp.is_some() {
                return Ok(());
            }
        }
        if let Some(input) = &node.spec.input_ref {
            let upstream = self.nodes_api.get_opt(&input.name).await?;
            if upstream.as_ref().is_none_or(|n| {
                n.metadata.deletion_timestamp.is_some()
                    || !n.spec.enabled
                    || n.uid() == node.uid()
                    || !compatible(&n.spec.settings, &node.spec.settings)
            }) {
                return self
                    .invalid(
                        &mut node,
                        "InvalidInput",
                        "Input MUST reference an available node with a compatible output.",
                    )
                    .await;
            }
        }
        let generation = node.metadata.generation.unwrap_or(1);
        let previous = node.status.clone();
        let status = node.status.get_or_insert_default();
        let Some(session_id) = self.active_session(&host) else {
            status.phase = Some(NodePhase::Disconnected);
            status.conditions.push(Condition {
                last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
                observed_generation: Some(generation),
                type_: "Ready".into(),
                status: "False".into(),
                reason: "HostOffline".into(),
                message: "Waiting for host registration.".into(),
            });
            status.conditions.push(Condition {
                last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
                observed_generation: Some(generation),
                type_: "Updated".into(),
                status: "False".into(),
                reason: "Reconciling".into(),
                message: "Waiting for host registration.".into(),
            });
            return self.node_status(&mut node, &previous).await;
        };
        let already_applied = status.observed_generation == Some(generation)
            && status.acknowledged_restart_token == node.spec.restart_token;
        if already_applied {
            return Ok(());
        }
        status.phase = Some(NodePhase::Pending);
        status.message = None;
        status.conditions.push(Condition {
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: Some(generation),
            type_: "Ready".into(),
            status: "False".into(),
            reason: "Reconciling".into(),
            message: "Waiting for runtime acknowledgement.".into(),
        });
        status.conditions.push(Condition {
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: Some(generation),
            type_: "Updated".into(),
            status: "False".into(),
            reason: "Reconciling".into(),
            message: "Waiting for runtime acknowledgement.".into(),
        });
        self.node_status(&mut node, &previous).await?;
        let message = RegistrationServerMessage::Apply {
            identity: ApplyIdentity {
                target: identity(&node.metadata)?,
                host_uid: host_uid.clone(),
                session_id: session_id.clone(),
                generation,
                restart_token: node.spec.restart_token.clone(),
                operation_id: uuid::Uuid::new_v4().to_string(),
            },
            spec: Box::new(node.spec),
        };
        self.dispatch(&host_uid, &session_id, message);
        Ok(())
    }
    fn dispatch(&mut self, host_uid: &str, session: &str, message: RegistrationServerMessage) {
        if !self.connections.send(host_uid, session, message)
            && self.connections.contains(host_uid, session)
        {
            self.close(host_uid);
        }
    }
    async fn report(
        &mut self,
        host_uid: &str,
        session_id: &str,
        message: RegistrationClientMessage,
    ) -> Result<(), Error> {
        let Some(connection) = self.connections.host(host_uid) else {
            return Ok(());
        };
        let Some(mut host) = self.hosts_api.get_opt(&connection.name).await? else {
            self.close(host_uid);
            return Ok(());
        };
        if !current_session(&host, host_uid, session_id)
            || !self.connections.contains(host_uid, session_id)
        {
            return Ok(());
        }
        match message {
            RegistrationClientMessage::Heartbeat {
                host_uid: uid,
                session_id: id,
            } if uid == host_uid && id == session_id => {
                let previous = host.status.clone();
                let status = host.status.get_or_insert_default();
                status.last_heartbeat = Some(Utc::now());
                status.presence = Some(Presence::Online);
                self.host_status(&mut host, &previous).await?;
            }
            RegistrationClientMessage::Acknowledgement {
                identity,
                operation: Operation::Apply,
                success,
                message,
            } => {
                if identity.host_uid != host_uid
                    || identity.session_id != session_id
                {
                    return Ok(());
                }
                let Some(mut node) = self.nodes_api.get_opt(&identity.target.name).await? else {
                    return Ok(());
                };
                if !matches_node(&node, &identity)
                    || node
                        .owner_references()
                        .iter()
                        .find(|o| o.controller == Some(true))
                        .is_none_or(|o| {
                            o.uid != host_uid
                                || o.name != host.name_any()
                                || o.kind != "SuntitleHost"
                                || o.api_version != "control.suntitle.io/v1alpha1"
                        })
                {
                    return Ok(());
                }
                let previous = node.status.clone();
                let status = node.status.get_or_insert_default();
                status.message = message.clone();
                status.phase = Some(if success {
                    if node.spec.enabled {
                        NodePhase::Running
                    } else {
                        NodePhase::Disabled
                    }
                } else {
                    NodePhase::Failed
                });
                if success {
                    status.observed_generation = Some(identity.generation);
                    status.acknowledged_restart_token = identity.restart_token.clone();
                    status.reporting_session = Some(session_id.into());
                }
                status.conditions.push(Condition {
                    last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
                    observed_generation: Some(identity.generation),
                    type_: "Updated".into(),
                    status: (if success { "True" } else { "False" }).into(),
                    reason: (if success {
                        "DesiredStateApplied"
                    } else {
                        "RuntimeFailed"
                    })
                    .into(),
                    message: (message.as_deref().unwrap_or(if success {
                        "Runtime acknowledged the desired specification."
                    } else {
                        "Runtime could not apply the desired specification."
                    }))
                    .into(),
                });
                status.conditions.push(Condition {
                    last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
                    observed_generation: Some(identity.generation),
                    type_: "Ready".into(),
                    status: (if success && node.spec.enabled {
                        "True"
                    } else {
                        "False"
                    })
                    .into(),
                    reason: (if success {
                        "DesiredStateApplied"
                    } else {
                        "RuntimeFailed"
                    })
                    .into(),
                    message: (message
                        .as_deref()
                        .unwrap_or("Runtime reported its current state."))
                    .into(),
                });
                self.node_status(&mut node, &previous).await?;
                if !success {
                    // A new attempt MUST remain distinct from a duplicate delivery.
                    self.triggers
                        .insert(Key::Node(identity.target.name, identity.target.uid));
                }
            }
            _ => {}
        }
        Ok(())
    }
}
fn current_session(host: &SuntitleHost, uid: &str, session: &str) -> bool {
    host.uid().as_deref() == Some(uid)
        && host.metadata.deletion_timestamp.is_none()
        && host
            .status
            .as_ref()
            .and_then(|status| status.session_id.as_deref())
            == Some(session)
}

fn heartbeat_expired(host: &SuntitleHost, now: chrono::DateTime<Utc>) -> bool {
    host.status
        .as_ref()
        .and_then(|status| status.last_heartbeat)
        .is_none_or(|heartbeat| {
            now.signed_duration_since(heartbeat)
                >= chrono::Duration::from_std(EXPIRY)
                    .expect("heartbeat expiry fits chrono duration")
        })
}

fn identity(metadata: &kube::core::ObjectMeta) -> Result<ResourceIdentity, Error> {
    Ok(ResourceIdentity {
        namespace: metadata.namespace.clone().ok_or("namespace missing")?,
        name: metadata.name.clone().ok_or("name missing")?,
        uid: metadata.uid.clone().ok_or("UID missing")?,
    })
}
fn relevant(metadata: &kube::core::ObjectMeta) -> Value {
    json!([
        metadata.uid,
        metadata.generation,
        metadata.deletion_timestamp,
        metadata.owner_references,
        metadata.finalizers
    ])
}
fn matches_node(node: &SuntitleNode, id: &ApplyIdentity) -> bool {
    node.uid().as_deref() == Some(&id.target.uid)
        && node.namespace().as_deref() == Some(&id.target.namespace)
        && node.name_any() == id.target.name
        && node.metadata.generation == Some(id.generation)
        && node.spec.restart_token == id.restart_token
        && node.metadata.deletion_timestamp.is_none()
}
fn compatible(input: &NodeSettings, output: &NodeSettings) -> bool {
    matches!(
        (input, output),
        (NodeSettings::AudioSource(_), NodeSettings::Transcriber(_))
            | (
                NodeSettings::Transcriber(_),
                NodeSettings::Translator(_) | NodeSettings::Display(_)
            )
            | (NodeSettings::Translator(_), NodeSettings::Display(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> State {
        let client =
            Client::try_from(kube::Config::new("http://127.0.0.1:9".parse().unwrap())).unwrap();
        State {
            hosts_api: Api::namespaced(client.clone(), "test"),
            nodes_api: Api::namespaced(client, "test"),

            resync: false,
            hosts_synced: false,
            nodes_synced: false,
            triggers: Triggers {
                namespace: "test".into(),
                hosts: mpsc::unbounded_channel().0,
                nodes: mpsc::unbounded_channel().0,
            },
            connections: Connections::default(),
        }
    }
    fn node() -> SuntitleNode {
        serde_json::from_value(json!({"apiVersion":"control.suntitle.io/v1alpha1","kind":"SuntitleNode","metadata":{"namespace":"test","name":"screen","uid":"node-uid","generation":1,"resourceVersion":"opaque"},"spec":{"hostRef":{"name":"host"},"name":"Screen","settings":{"display":{"maxLines":3,"fontScale":1.0,"showOriginal":false}}}})).unwrap()
    }
    #[tokio::test]
    async fn relist_gates_dispatch_and_requests_api_resynchronization() {
        let mut state = state();
        state.hosts_synced = true;
        state.node_event(&watcher::Event::Init).await.unwrap();
        state
            .node_event(&watcher::Event::InitApply(node()))
            .await
            .unwrap();
        assert!(!state.synced());
        assert!(!state.resync);
        state.node_event(&watcher::Event::InitDone).await.unwrap();
        assert!(state.synced());
        assert!(state.resync);
        state.host_event(&watcher::Event::Init).await.unwrap();
        assert!(!state.synced());
        state.host_event(&watcher::Event::InitDone).await.unwrap();
        assert!(state.synced());
    }

    #[test]
    fn status_updates_are_quiet_and_lifecycle_changes_pass_the_gate() {
        let mut changed = node();
        changed.status = Some(NodeStatus::default());
        changed.metadata.resource_version = Some("opaque-new-token".into());
        assert_eq!(predicate(&changed), predicate(&node()));
        changed.metadata.uid = Some("replacement".into());
        assert_ne!(predicate(&changed), predicate(&node()));
        let mut finalized = node();
        finalized.metadata.finalizers = Some(vec![NODE_FINALIZER.into()]);
        assert_ne!(predicate(&finalized), predicate(&node()));
    }

    #[tokio::test]
    async fn dependency_queries_read_current_api_resources() {
        use warp::Filter;
        let mut owned = node();
        owned.spec.host_ref.name = "different-host".into();
        owned.metadata.owner_references = Some(vec![
            k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference {
                api_version: "control.suntitle.io/v1alpha1".into(),
                kind: "SuntitleHost".into(),
                name: "host".into(),
                uid: "host-uid".into(),
                controller: Some(true),
                block_owner_deletion: Some(true),
            },
        ]);
        let resources = Arc::new(std::sync::Mutex::new(vec![owned]));
        let served = resources.clone();
        let route = warp::any().map(move || {
            warp::reply::json(&json!({
                "apiVersion":"control.suntitle.io/v1alpha1", "kind":"SuntitleNodeList",
                "metadata":{"resourceVersion":"opaque"}, "items": *served.lock().unwrap(),
            }))
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(warp::serve(route).incoming(listener).run());
        let mut state = state();
        let client = Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        state.nodes_api = Api::namespaced(client, "test");
        let (tx, mut rx) = mpsc::unbounded_channel();
        state.triggers.nodes = tx;
        state.schedule_host_nodes("host", "host-uid").await.unwrap();
        assert_eq!(rx.try_recv().unwrap().name, "screen");
        state.dependents("upstream").await.unwrap();
        assert!(rx.try_recv().is_err());
        let mut replacement = node();
        replacement.metadata.name = Some("new-dependent".into());
        replacement.spec.input_ref = Some(ResourceRef {
            name: "upstream".into(),
        });
        *resources.lock().unwrap() = vec![replacement];
        state.dependents("upstream").await.unwrap();
        assert_eq!(rx.try_recv().unwrap().name, "new-dependent");
        state
            .schedule_host_nodes("host", "replacement-host-uid")
            .await
            .unwrap();
        assert_eq!(rx.try_recv().unwrap().name, "new-dependent");
        resources.lock().unwrap().clear();
        state.dependents("upstream").await.unwrap();
        assert!(rx.try_recv().is_err());
        server.abort();
    }

    #[test]
    fn heartbeat_expiry_and_session_identity_use_resource_status() {
        let mut host = SuntitleHost::new(
            "host",
            SuntitleHostSpec {
                name: "Host".into(),
            },
        );
        host.metadata.uid = Some("uid".into());
        let now = Utc::now();
        assert!(heartbeat_expired(&host, now));
        let status = host.status.get_or_insert_default();
        status.last_heartbeat = Some(now);
        status.session_id = Some("current".into());
        assert!(!heartbeat_expired(&host, now));
        assert!(heartbeat_expired(
            &host,
            now + chrono::Duration::seconds(45)
        ));
        assert!(current_session(&host, "uid", "current"));
        assert!(!current_session(&host, "uid", "old"));
        assert!(!current_session(&host, "old-uid", "current"));
    }

    #[tokio::test]
    async fn outbound_overflow_closes_session_and_reschedules_owned_nodes() {
        let mut state = state();
        let connection = state.connections.open(
            ResourceIdentity {
                namespace: "test".into(),
                name: "host".into(),
                uid: "host-uid".into(),
            },
            "session".into(),
        );
        for _ in 0..32 {
            state.dispatch(
                "host-uid",
                "session",
                RegistrationServerMessage::Error {
                    message: "queued".into(),
                },
            );
        }
        assert!(!*connection.closed.borrow());
        state.dispatch(
            "host-uid",
            "session",
            RegistrationServerMessage::Error {
                message: "overflow".into(),
            },
        );
        assert!(*connection.closed.borrow());
        assert!(state.connections.hosts().is_empty());
        assert!(state.resync);
    }

    #[test]
    fn stale_incarnation_generation_token_and_deletion_reject_reports() {
        let mut node = node();
        let mut id = ApplyIdentity {
            target: identity(&node.metadata).unwrap(),
            host_uid: "host".into(),
            session_id: "session".into(),
            generation: 1,
            restart_token: None,
            operation_id: "op".into(),
        };
        assert!(matches_node(&node, &id));
        id.target.uid = "old".into();
        assert!(!matches_node(&node, &id));
        id.target.uid = "node-uid".into();
        id.generation = 2;
        assert!(!matches_node(&node, &id));
        id.generation = 1;
        id.restart_token = Some("old".into());
        assert!(!matches_node(&node, &id));
        id.restart_token = None;
        node.metadata.deletion_timestamp =
            Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            ));
        assert!(!matches_node(&node, &id));
    }
}

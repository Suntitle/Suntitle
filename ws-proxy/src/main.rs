// #![deny(warnings)]
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use chrono::Utc;
use futures_util::{SinkExt, StreamExt, TryFutureExt};
use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use utoipa_actix_web::AppExt;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::reply::Reply;
use warp::ws::{Message, WebSocket};
use ws_proxy::api::models::{Host, HostStatus, NodeStatus};
use ws_proxy::registration::{RegistrationClientMessage, RegistrationServerMessage};
use ws_proxy::{ControlPlaneState, HostControlRegistry, HostRegistry, NodeRegistry};

/// Our global unique user id counter.
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// Our state of currently connected clients.
type Transcribers = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;
type Translators = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;
type Displays = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    pretty_env_logger::init();
    let state = ControlPlaneState::default();
    let routes = proxy_with_state(state.clone()).await;
    let control_api = actix_web::HttpServer::new(move || {
        let state = state.clone();
        actix_web::App::new()
            .into_utoipa_app()
            .configure(move |config| ws_proxy::api::configure_with_state(config, state))
            .into_app()
    })
    .bind(("0.0.0.0", 3031))?
    .run();

    eprintln!("Suntitle WebSocket proxy listening on 0.0.0.0:3030");
    eprintln!("Suntitle REST API listening on 0.0.0.0:3031");
    tokio::select! {
        result = control_api => result,
        _ = warp::serve(routes).run(([0, 0, 0, 0], 3030)) => Ok(()),
    }
}

#[cfg(test)]
async fn proxy() -> BoxedFilter<(impl Reply,)> {
    proxy_with_hosts(HostRegistry::default()).await
}

#[cfg(test)]
async fn proxy_with_hosts(hosts: HostRegistry) -> BoxedFilter<(impl Reply,)> {
    proxy_with_state(ControlPlaneState {
        hosts,
        ..ControlPlaneState::default()
    })
    .await
}

async fn proxy_with_state(control_plane: ControlPlaneState) -> BoxedFilter<(impl Reply,)> {
    let state = Transcribers::default();
    let transcribers = warp::any().map(move || state.clone());

    let state = Translators::default();
    let translators = warp::any().map(move || state.clone());

    let state = Displays::default();
    let displays = warp::any().map(move || state.clone());

    let hosts = control_plane.hosts;
    let nodes = control_plane.nodes;
    let host_controls = control_plane.host_controls;
    let host_registry = warp::any().map(move || hosts.clone());
    let node_registry = warp::any().map(move || nodes.clone());
    let control_registry = warp::any().map(move || host_controls.clone());

    // GET /transcribe -> transcriber serving
    let transcribe = warp::path("transcribe")
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(transcribers.clone())
        .and(translators.clone())
        .map(|ws: warp::ws::Ws, transcribers, translators| {
            // This will call our function if the handshake succeeds.
            ws.on_upgrade(move |socket| transcriber_connected(socket, transcribers, translators))
        });

    // GET /translate -> translator serving
    let translate = warp::path("translate")
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(translators)
        .and(displays.clone())
        .map(|ws: warp::ws::Ws, translators, displays| {
            // This will call our function if the handshake succeeds.
            ws.on_upgrade(move |socket| translator_connected(socket, translators, displays))
        });

    // GET /display -> display translation
    let display =
        warp::path("display")
            .and(warp::ws())
            .and(displays)
            .map(|ws: warp::ws::Ws, translators| {
                // This will call our function if the handshake succeeds.
                ws.on_upgrade(move |socket| display_connected(socket, translators))
            });

    // GET /register -> Diamant host registration. Persistence is intentionally
    // mocked until the control-plane host store is implemented.
    let register = warp::path("register")
        .and(warp::path::end())
        .and(warp::ws())
        .and(host_registry)
        .and(node_registry)
        .and(control_registry)
        .map(|ws: warp::ws::Ws, hosts, nodes, controls| {
            ws.on_upgrade(move |socket| registration_connected(socket, hosts, nodes, controls))
        });

    // GET / -> audio data stream
    let index = warp::path::end().and(warp::ws()).and(transcribers).map(
        |ws: warp::ws::Ws, transcribers: Transcribers| {
            // This will call our function if the handshake succeeds.
            ws.on_upgrade(async move |socket| {
                let (_, mut audio_read) = socket.split();
                while let Some(Ok(message)) = audio_read.next().await {
                    if message.is_binary() {
                        transcriber_message(message, &transcribers).await;
                    }
                }
            })
        },
    );

    index
        .or(register)
        .or(transcribe)
        .or(translate)
        .or(display)
        .boxed()
}

async fn registration_connected(
    ws: WebSocket,
    hosts: HostRegistry,
    nodes: NodeRegistry,
    controls: HostControlRegistry,
) {
    let (mut write, mut read) = ws.split();

    let Some(Ok(message)) = read.next().await else {
        return;
    };

    let response = match message
        .to_str()
        .ok()
        .and_then(|json| serde_json::from_str::<RegistrationClientMessage>(json).ok())
    {
        Some(RegistrationClientMessage::RegisterHost {
            id,
            name,
            ip,
            version,
            metadata,
        }) if !name.trim().is_empty() && !version.trim().is_empty() => {
            let host = Host {
                id,
                name,
                ip,
                version,
                status: HostStatus::Online,
                last_seen_at: Utc::now(),
                metadata,
            };
            hosts.write().await.insert(host.id, host.clone());
            eprintln!(
                "registered host: {} version={} ip={} id={}",
                host.name, host.version, host.ip, host.id
            );
            RegistrationServerMessage::Registered { host }
        }
        _ => RegistrationServerMessage::Error {
            message: "expected a register_host message with an id, name, IP, and version"
                .to_owned(),
        },
    };

    if let Ok(json) = serde_json::to_string(&response) {
        let _ = write.send(Message::text(json)).await;
    }

    let registered_host_id = match &response {
        RegistrationServerMessage::Registered { host } => Some(host.id),
        _ => None,
    };

    let (control_tx, mut control_rx) = mpsc::unbounded_channel();
    if let Some(host_id) = registered_host_id {
        controls.write().await.insert(host_id, control_tx.clone());
        let snapshot: Vec<_> = nodes
            .read()
            .await
            .values()
            .filter(|node| node.host_id == host_id)
            .map(|node| RegistrationServerMessage::ApplyNode {
                node_id: node.node.id(),
                revision: node.revision,
                update: node.update.clone(),
            })
            .collect();
        for command in snapshot {
            if let Ok(json) = serde_json::to_string(&command)
                && write.send(Message::text(json)).await.is_err()
            {
                break;
            }
        }
    }

    loop {
        tokio::select! {
        result = read.next() => match result {
            None => break,
            Some(Ok(message)) if message.is_close() => break,
            Some(Ok(message)) if message.is_ping() => {
                if write
                    .send(Message::pong(message.as_bytes().to_vec()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Some(Ok(message)) if message.is_text() => {
                match serde_json::from_slice(message.as_bytes()) {
                    Ok(RegistrationClientMessage::HealthOk { host_id, ip, metadata }) if Some(host_id) == registered_host_id => {
                        if let Some(host) = hosts.write().await.get_mut(&host_id) {
                            host.last_seen_at = Utc::now();
                            host.status = HostStatus::Online;
                            host.ip = ip;
                            host.metadata = metadata;
                        }
                        eprintln!("host health OK: id={host_id}");
                    }
                    Ok(RegistrationClientMessage::NodeStatus { host_id, node_id, revision, status, message: _ }) if Some(host_id) == registered_host_id => {
                        if let Some(node) = nodes.write().await.get_mut(&node_id)
                            && node.host_id == host_id && node.revision == revision
                        {
                            let api_status = match status {
                                ws_proxy::registration::NodeRuntimeStatus::Running => NodeStatus::Connected,
                                ws_proxy::registration::NodeRuntimeStatus::Failed => NodeStatus::Degraded,
                                ws_proxy::registration::NodeRuntimeStatus::Starting | ws_proxy::registration::NodeRuntimeStatus::Stopped => NodeStatus::Disconnected,
                            };
                            node.node.set_status(api_status);
                        }
                    }
                    _ => {}
                }
            }
            Some(Ok(_)) => {}
            Some(Err(error)) => {
                eprintln!("host registration websocket error: {error}");
                break;
            }
        },
        command = control_rx.recv() => {
            let Some(command) = command else { break; };
            let Ok(json) = serde_json::to_string(&command) else { continue; };
            if write.send(Message::text(json)).await.is_err() { break; }
        }
        }
    }

    if let Some(host_id) = registered_host_id {
        let should_remove = controls
            .read()
            .await
            .get(&host_id)
            .is_some_and(|current| current.same_channel(&control_tx));
        if should_remove {
            controls.write().await.remove(&host_id);
            if let Some(host) = hosts.write().await.get_mut(&host_id) {
                host.status = HostStatus::Offline;
            }
            nodes
                .write()
                .await
                .retain(|_, node| node.host_id != host_id);
        }
        eprintln!("host disconnected: id={host_id}");
    }
}

async fn translator_connected(ws: WebSocket, translators: Translators, displays: Displays) {
    // Use a counter to assign a new unique ID for this translator.
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    eprintln!("new translator: {}", id);

    // Split the socket into a sender and receive of messages.
    let (mut translator_write, mut translator_read) = ws.split();

    // Use an unbounded channel to handle buffering and flushing of messages
    // to the websocket...
    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    tokio::task::spawn(async move {
        while let Some(message) = rx.next().await {
            translator_write
                .send(message)
                .unwrap_or_else(|e| {
                    eprintln!("translator websocket send error: {}", e);
                })
                .await;
        }
    });

    // Save the sender in our list of connected transcribers.
    translators.write().await.insert(id, tx);

    // Return a `Future` that is basically a state machine managing
    // this specific transcribers's connection.

    // Every time the transcriber sends a message, broadcast it to
    // all other transcribers...
    while let Some(result) = translator_read.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("translator websocket error(uid={}): {}", id, e);
                break;
            }
        };
        display_message(msg, &displays).await;
    }

    // transcriber_read stream will keep processing as long as the transcriber stays
    // connected. Once they disconnect, then...
    translators.write().await.remove(&id);
}

async fn transcriber_connected(
    ws: WebSocket,
    transcribers: Transcribers,
    translators: Translators,
) {
    // Use a counter to assign a new unique ID for this transcriber.
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    eprintln!("new transcriber: {}", id);

    // Split the socket into a sender and receive of messages.
    let (mut transcriber_write, mut transcriber_read) = ws.split();

    // Use an unbounded channel to handle buffering and flushing of messages
    // to the websocket...
    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    tokio::task::spawn(async move {
        while let Some(message) = rx.next().await {
            transcriber_write
                .send(message)
                .unwrap_or_else(|e| {
                    eprintln!("transcriber websocket send error: {}", e);
                })
                .await;
        }
    });

    // Save the sender in our list of connected transcribers.
    transcribers.write().await.insert(id, tx);

    // Return a `Future` that is basically a state machine managing
    // this specific transcribers's connection.

    // Every time the transcriber sends a message, broadcast it to
    // all other transcribers...
    while let Some(result) = transcriber_read.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("transcriber websocket error(uid={}): {}", id, e);
                break;
            }
        };
        translator_message(msg, &translators).await;
    }

    // transcriber_read stream will keep processing as long as the transcriber stays
    // connected. Once they disconnect, then...
    transcribers.write().await.remove(&id);
}

async fn display_connected(ws: WebSocket, displays: Displays) {
    // Use a counter to assign a new unique ID for this translator.
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    eprintln!("new display: {}", id);

    // Split the socket into a sender and receive of messages.
    let (mut display_write, mut display_read) = ws.split();

    // Use an unbounded channel to handle buffering and flushing of messages
    // to the websocket...
    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    tokio::task::spawn(async move {
        while let Some(message) = rx.next().await {
            display_write
                .send(message)
                .unwrap_or_else(|e| {
                    eprintln!("display websocket send error: {}", e);
                })
                .await;
        }
    });

    // Save the sender in our list of connected displays.
    displays.write().await.insert(id, tx);

    while let Some(result) = display_read.next().await {
        match result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("display websocket error(uid={}): {}", id, e);
                break;
            }
        };
    }

    // display_read stream will keep processing as long as the transcriber stays
    // connected. Once they disconnect, then...
    displays.write().await.remove(&id);
}

async fn transcriber_message(msg: Message, transcribers: &Transcribers) {
    for (&_uid, tx) in transcribers.read().await.iter() {
        if let Err(_disconnected) = tx.send(msg.clone()) {
            // The tx is disconnected, our `user_disconnected` code
            // should be happening in another task, nothing more to
            // do here.
        }
    }
}

async fn translator_message(msg: Message, translators: &Translators) {
    for (&_uid, tx) in translators.read().await.iter() {
        if let Err(_disconnected) = tx.send(msg.clone()) {
            // The tx is disconnected, our `user_disconnected` code
            // should be happening in another task, nothing more to
            // do here.
        }
    }
}

async fn display_message(msg: Message, displays: &Displays) {
    for (&_uid, tx) in displays.read().await.iter() {
        if let Err(_disconnected) = tx.send(msg.clone()) {
            // The tx is disconnected, our `user_disconnected` code
            // should be happening in another task, nothing more to
            // do here.
        }
    }
}

#[cfg(test)]
mod tests {
    use warp::filters::ws::Message;

    use crate::{
        ControlPlaneState, HostRegistry, HostStatus, RegistrationClientMessage, proxy,
        proxy_with_hosts, proxy_with_state,
    };

    #[tokio::test]
    async fn test_transcriber_connected() {
        warp::test::ws()
            .path("/transcribe")
            .handshake(proxy().await)
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn test_translator_connected() {
        warp::test::ws()
            .path("/translate")
            .handshake(proxy().await)
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn test_audio_connected() {
        warp::test::ws()
            .path("/")
            .handshake(proxy().await)
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn test_display_connected() {
        warp::test::ws()
            .path("/display")
            .handshake(proxy().await)
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn test_diamant_registers_a_host() {
        let host_id = uuid::Uuid::parse_str("20000000-0000-4000-8000-000000000001").unwrap();
        let hosts = HostRegistry::default();
        let mut diamant = warp::test::ws()
            .path("/register")
            .handshake(proxy_with_hosts(hosts.clone()).await)
            .await
            .expect("handshake");
        let request = ws_proxy::registration::RegistrationClientMessage::RegisterHost {
            id: host_id,
            name: "studio-host".to_owned(),
            ip: "192.0.2.10".parse().unwrap(),
            version: "0.1.0".to_owned(),
            metadata: None,
        };

        diamant
            .send(Message::text(
                serde_json::to_string(&request).expect("request serializes"),
            ))
            .await;

        let response = diamant.recv().await.expect("registration response");
        let response: ws_proxy::registration::RegistrationServerMessage =
            serde_json::from_slice(response.as_bytes()).expect("valid response");
        let ws_proxy::registration::RegistrationServerMessage::Registered { host } = response
        else {
            panic!("expected successful registration");
        };
        assert_eq!(host.id, host_id);
        assert_eq!(host.name, "studio-host");
        assert_eq!(host.ip.to_string(), "192.0.2.10");
        assert_eq!(host.version, "0.1.0");
        assert!(matches!(
            host.status,
            ws_proxy::api::models::HostStatus::Online
        ));
        assert!(host.metadata.is_none());

        let registered = hosts.read().await.get(&host_id).cloned().unwrap();
        assert!(matches!(registered.status, HostStatus::Online));

        diamant
            .send(Message::text(
                serde_json::to_string(&RegistrationClientMessage::HealthOk {
                    host_id,
                    ip: "192.0.2.11".parse().unwrap(),
                    metadata: Some(std::collections::BTreeMap::from([(
                        "arch".to_owned(),
                        serde_json::Value::String("aarch64".to_owned()),
                    )])),
                })
                .unwrap(),
            ))
            .await;

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let updated = hosts.read().await.get(&host_id).cloned().unwrap();
                if updated.ip.to_string() == "192.0.2.11" {
                    assert!(updated.metadata.unwrap().contains_key("arch"));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("health update stored");

        diamant.send(Message::close()).await;
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let status = hosts.read().await.get(&host_id).unwrap().status;
                if status == HostStatus::Offline {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("disconnect stored");
    }

    #[tokio::test]
    async fn registration_replays_the_hosts_desired_nodes() {
        let host_id = uuid::Uuid::new_v4();
        let node_id = uuid::Uuid::new_v4();
        let update: ws_proxy::api::models::NodeUpdate = serde_json::from_value(serde_json::json!({
            "type":"display", "name":"screen", "description":null, "enabled":true,
            "settings":{"maxLines":3,"fontScale":1.0,"showOriginal":false,"theme":"dark"},
            "inputNodeId":null
        }))
        .unwrap();
        let now = chrono::Utc::now();
        let host = ws_proxy::api::models::Host {
            id: host_id,
            name: "host".to_owned(),
            ip: "192.0.2.10".parse().unwrap(),
            version: "0.1.0".to_owned(),
            status: HostStatus::Offline,
            last_seen_at: now,
            metadata: None,
        };
        let node = ws_proxy::api::models::Node::Display(ws_proxy::api::models::DisplayNode {
            base: ws_proxy::api::models::NodeBase {
                id: node_id,
                node_type: ws_proxy::api::models::NodeType::Display,
                name: "screen".to_owned(),
                description: None,
                enabled: true,
                status: ws_proxy::api::models::NodeStatus::Disconnected,
                host,
                connections: Vec::new(),
                created_at: now,
                updated_at: now,
            },
            variant: ws_proxy::api::models::DisplayVariant {
                node_type: ws_proxy::api::models::NodeType::Display,
                settings: ws_proxy::api::models::DisplaySettings {
                    max_lines: 3,
                    font_scale: 1.0,
                    show_original: false,
                    theme: ws_proxy::api::models::DisplayTheme::Dark,
                },
            },
        });
        let state = ControlPlaneState::default();
        state.nodes.write().await.insert(
            node_id,
            ws_proxy::DesiredNode {
                host_id,
                revision: 9,
                update,
                node,
            },
        );
        let mut diamant = warp::test::ws()
            .path("/register")
            .handshake(proxy_with_state(state.clone()).await)
            .await
            .expect("handshake");
        let register = RegistrationClientMessage::RegisterHost {
            id: host_id,
            name: "host".to_owned(),
            ip: "192.0.2.10".parse().unwrap(),
            version: "0.1.0".to_owned(),
            metadata: None,
        };
        diamant
            .send(Message::text(serde_json::to_string(&register).unwrap()))
            .await;
        let registered = diamant.recv().await.unwrap();
        assert!(matches!(
            serde_json::from_slice(registered.as_bytes()).unwrap(),
            ws_proxy::registration::RegistrationServerMessage::Registered { .. }
        ));
        let replay = diamant.recv().await.unwrap();
        assert!(matches!(serde_json::from_slice(replay.as_bytes()).unwrap(),
            ws_proxy::registration::RegistrationServerMessage::ApplyNode { node_id: replayed, revision: 9, .. } if replayed == node_id));

        diamant.send(Message::close()).await;
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if state.nodes.read().await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("host disconnect removes its nodes");
    }

    #[tokio::test]
    async fn full() {
        let proxy = proxy().await;
        let mut audio = warp::test::ws()
            .path("/")
            .handshake(proxy.clone())
            .await
            .expect("handshake");
        let mut transcriber = warp::test::ws()
            .path("/transcribe")
            .handshake(proxy.clone())
            .await
            .expect("handshake");
        let mut translator = warp::test::ws()
            .path("/translate")
            .handshake(proxy.clone())
            .await
            .expect("handshake");
        let mut display = warp::test::ws()
            .path("/display")
            .handshake(proxy)
            .await
            .expect("handshake");

        // audio communication
        let msg = Message::binary("binary");
        audio.send(msg.clone()).await;
        assert_eq!(transcriber.recv().await.expect("message"), msg);

        // transcriber communication
        let transcription = Message::text("transcription");
        transcriber.send(transcription.clone()).await;
        assert_eq!(translator.recv().await.expect("message"), transcription);

        // display communication
        let translation = Message::text("translation");
        translator.send(translation.clone()).await;
        assert_eq!(display.recv().await.expect("message"), translation);
    }
}

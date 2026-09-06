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
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::reply::Reply;
use warp::ws::{Message, WebSocket};
use ws_proxy::api::models::{Host, HostStatus};
use ws_proxy::registration::{RegistrationClientMessage, RegistrationServerMessage};
use ws_proxy::storage::{Mutation, PostgresStore, Store};

/// Our global unique user id counter.
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// Our state of currently connected clients.
type Transcribers = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;
type Translators = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;
type Displays = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    pretty_env_logger::init();
    let url = ws_proxy::storage::postgres::database_url()?;
    let postgres = PostgresStore::connect(&url).await?;
    let events = postgres.proxy_events(&url).await?;
    let store: Store = Arc::new(postgres);
    let routes = proxy_with_store(store, events.receiver).await;
    eprintln!("Suntitle WebSocket proxy listening on 0.0.0.0:3030");
    tokio::select! {
        _ = warp::serve(routes).run(([0, 0, 0, 0], 3030)) => {},
        result = events.task => {
            result.map_err(std::io::Error::other)??;
            return Err(std::io::Error::other("control listener stopped"));
        }
    }
    Ok(())
}

#[cfg(test)]
async fn proxy() -> BoxedFilter<(impl Reply,)> {
    let (_, receiver) = tokio::sync::watch::channel(0);
    proxy_with_store(
        Arc::new(ws_proxy::storage::MemoryStore::default()),
        receiver,
    )
    .await
}

async fn proxy_with_store(
    store: Store,
    changes: tokio::sync::watch::Receiver<u64>,
) -> BoxedFilter<(impl Reply,)> {
    let state = Transcribers::default();
    let transcribers = warp::any().map(move || state.clone());

    let state = Translators::default();
    let translators = warp::any().map(move || state.clone());

    let state = Displays::default();
    let displays = warp::any().map(move || state.clone());

    let storage = warp::any().map(move || store.clone());
    let notifications = warp::any().map(move || changes.clone());

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

    let register = warp::path("register")
        .and(warp::path::end())
        .and(warp::ws())
        .and(storage)
        .and(notifications)
        .map(|ws: warp::ws::Ws, store, changes| {
            ws.on_upgrade(move |socket| registration_connected(socket, store, changes))
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
    store: Store,
    mut changes: tokio::sync::watch::Receiver<u64>,
) {
    let (mut write, mut read) = ws.split();
    let Ok(Some(Ok(message))) =
        tokio::time::timeout(std::time::Duration::from_secs(10), read.next()).await
    else {
        return;
    };
    let Ok(RegistrationClientMessage::RegisterHost {
        id,
        name,
        ip,
        version,
        metadata,
    }) = serde_json::from_slice(message.as_bytes())
    else {
        return;
    };
    if name.trim().is_empty() || version.trim().is_empty() {
        return;
    }
    let session = uuid::Uuid::new_v4();
    let host = Host {
        id,
        name,
        ip,
        version,
        metadata,
        status: HostStatus::Online,
        last_seen_at: Utc::now(),
    };
    if store
        .mutate(Mutation::Register(host.clone(), session))
        .await
        .is_err()
    {
        return;
    }
    let reply = RegistrationServerMessage::Registered { host };
    if write
        .send(Message::text(serde_json::to_string(&reply).unwrap()))
        .await
        .is_err()
    {
        let _ = store.mutate(Mutation::Disconnect(id, session)).await;
        return;
    }
    // The receiver is subscribed before this initial scan. Timer scans recover
    // notifications lost during listener reconnects and retry unacknowledged work.
    let mut retry = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        tokio::select! {
            result = read.next() => {
                match result {
                    Some(Ok(message)) if message.is_ping() => {
                        if write.send(Message::pong(message.as_bytes().to_vec())).await.is_err() { break; }
                    }
                    Some(Ok(message)) if message.is_text() => {
                        let mutation = match serde_json::from_slice(message.as_bytes()) {
                            Ok(RegistrationClientMessage::HealthOk { host_id, ip, metadata }) if host_id == id => Mutation::Health(id, session, ip, metadata),
                            Ok(RegistrationClientMessage::NodeStatus { host_id, node_id, revision, status, .. }) if host_id == id => Mutation::Status(id, session, node_id, revision, status),
                            _ => break,
                        };
                        if store.mutate(mutation).await.is_err() { break; }
                    }
                    Some(Ok(message)) if !message.is_close() => {},
                    _ => break,
                }
            }
            _ = retry.tick() => {
                if !dispatch(&store, id, session, &mut write).await { break; }
            }
            result = changes.changed() => {
                if result.is_err() { break; }
                if !dispatch(&store, id, session, &mut write).await { break; }
            }
        }
    }
    let _ = store.mutate(Mutation::Disconnect(id, session)).await;
}

async fn dispatch(
    store: &Store,
    host: uuid::Uuid,
    session: uuid::Uuid,
    write: &mut futures_util::stream::SplitSink<WebSocket, Message>,
) -> bool {
    let Ok(commands) = store.pending(host, session).await else {
        return false;
    };
    for command in commands {
        if write
            .send(Message::text(
                serde_json::to_string(&command.message).unwrap(),
            ))
            .await
            .is_err()
        {
            return false;
        }
    }
    true
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

    use crate::proxy;

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
    async fn registration_recovers_missed_notifications_and_retains_nodes() {
        use ws_proxy::api::models::{HostStatus, NodeUpdate};
        use ws_proxy::registration::{
            NodeRuntimeStatus, RegistrationClientMessage as Client,
            RegistrationServerMessage as Server,
        };
        use ws_proxy::storage::{MemoryStore, Mutation, Store};
        let store: Store = std::sync::Arc::new(MemoryStore::default());
        // Never publish a notification: dispatch must recover through its timer.
        let (_sender, receiver) = tokio::sync::watch::channel(0);
        let routes = super::proxy_with_store(store.clone(), receiver).await;
        let host_id = uuid::Uuid::new_v4();
        let register = Client::RegisterHost {
            id: host_id,
            name: "host".into(),
            ip: "192.0.2.10".parse().unwrap(),
            version: "test".into(),
            metadata: None,
        };
        let mut socket = warp::test::ws()
            .path("/register")
            .handshake(routes.clone())
            .await
            .unwrap();
        socket
            .send(Message::text(serde_json::to_string(&register).unwrap()))
            .await;
        assert!(matches!(
            serde_json::from_slice::<Server>(socket.recv().await.unwrap().as_bytes()).unwrap(),
            Server::Registered { .. }
        ));
        let node_id = uuid::Uuid::new_v4();
        let update: NodeUpdate = serde_json::from_value(serde_json::json!({"type":"display", "name":"screen", "enabled":true, "inputNodeId":null, "settings":{"maxLines":3,"fontScale":1.0,"showOriginal":false}})).unwrap();
        store.create_node(host_id, node_id, update).await.unwrap();
        for _ in 0..2 {
            let message = tokio::time::timeout(std::time::Duration::from_secs(4), socket.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                serde_json::from_slice::<Server>(message.as_bytes()).unwrap(),
                Server::ApplyNode { revision: 1, .. }
            ));
        }
        let status = Client::NodeStatus {
            host_id,
            node_id,
            revision: 1,
            status: NodeRuntimeStatus::Running,
            message: None,
        };
        socket
            .send(Message::text(serde_json::to_string(&status).unwrap()))
            .await;
        socket.send(Message::close()).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if store.snapshot().await.unwrap().hosts[&host_id].status == HostStatus::Offline {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(store.nodes().await.unwrap().len(), 1);
        let mut socket = warp::test::ws()
            .path("/register")
            .handshake(routes)
            .await
            .unwrap();
        socket
            .send(Message::text(serde_json::to_string(&register).unwrap()))
            .await;
        socket.recv().await.unwrap();
        let message = socket.recv().await.unwrap();
        assert!(matches!(
            serde_json::from_slice::<Server>(message.as_bytes()).unwrap(),
            Server::ApplyNode { revision: 2, .. }
        ));
        // Replaced sessions cannot mutate the host or the node.
        assert!(
            store
                .mutate(Mutation::Disconnect(host_id, uuid::Uuid::new_v4()))
                .await
                .is_err()
        );
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

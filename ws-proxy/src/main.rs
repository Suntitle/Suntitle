// #![deny(warnings)]
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use clap::Parser;
use futures_util::{SinkExt, StreamExt, TryFutureExt};
use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::reply::Reply;
use warp::ws::{Message, WebSocket};
use ws_proxy::controller::Controller;
use ws_proxy::registration::{Envelope, RegistrationClientMessage, RegistrationServerMessage};

#[derive(Parser)]
/// Proxy, Kubernetes, and extension publication configuration.
struct Args {
    /// Explicit credentials for Kubernetes resource watches and status updates.
    #[arg(long, env = "SUNTITLE_PROXY_KUBECONFIG")]
    kubeconfig: std::path::PathBuf,
    /// Namespace containing hosts, nodes, and extensions.
    #[arg(
        long,
        env = "SUNTITLE_KUBERNETES_NAMESPACE",
        default_value = "suntitle"
    )]
    namespace: String,
    /// Reachable registry authority and repository prefix for extension artifacts.
    #[arg(
        long,
        env = "SUNTITLE_EXTENSION_REGISTRY",
        default_value = "registry.localhost/suntitle/extensions"
    )]
    extension_registry: String,
    /// Explicit local-development permission to use HTTP for OCI transfers.
    #[arg(long, env = "SUNTITLE_EXTENSION_REGISTRY_PLAIN_HTTP")]
    extension_registry_plain_http: bool,
    /// PEM certificate bundle trusted for registry TLS.
    #[arg(long, env = "SUNTITLE_EXTENSION_REGISTRY_CA_FILE")]
    extension_registry_ca_file: Option<std::path::PathBuf>,
    /// JSON map of registry authorities to username/password credentials.
    #[arg(long, env = "SUNTITLE_EXTENSION_CREDENTIALS_FILE")]
    extension_credentials_file: Option<std::path::PathBuf>,
    /// Publication workers, independently bounded from controller concurrency.
    #[arg(long, env = "SUNTITLE_EXTENSION_CONCURRENCY", default_value_t = 2)]
    extension_concurrency: usize,
    /// Maximum duration of one preparation and publication attempt.
    #[arg(
        long,
        env = "SUNTITLE_EXTENSION_TIMEOUT_SECONDS",
        default_value_t = 600
    )]
    extension_timeout_seconds: u64,
}

/// Our global unique user id counter.
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// Our state of currently connected clients.
type Transcribers = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;
type Translators = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;
type Displays = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    pretty_env_logger::init();
    let args = Args::parse();
    let client = ws_proxy::kubernetes::client_from_kubeconfig(&args.kubeconfig)
        .await
        .map_err(std::io::Error::other)?;
    let credentials: std::collections::BTreeMap<String, ws_proxy::controller::RegistryCredential> =
        match &args.extension_credentials_file {
            Some(path) => {
                serde_json::from_slice(&std::fs::read(path)?).map_err(std::io::Error::other)?
            }
            None => Default::default(),
        };
    let authority = args
        .extension_registry
        .split('/')
        .next()
        .unwrap_or_default();
    let authentication = credentials
        .get(authority)
        .map(|credential| credential.authentication())
        .unwrap_or(oci_artifact::RegistryAuth::Anonymous);
    let config = ws_proxy::controller::PublicationConfig {
        repository: args.extension_registry,
        authentication,
        source_authentication: credentials
            .into_iter()
            .map(|(host, credential)| (host, credential.authentication()))
            .collect(),
        transport: oci_artifact::client::TransportOptions {
            plain_http: args.extension_registry_plain_http,
            ca_file: args.extension_registry_ca_file,
            ..Default::default()
        },
        concurrency: args.extension_concurrency,
        task_timeout: std::time::Duration::from_secs(args.extension_timeout_seconds),
    };
    let (controller, task) = Controller::start_with_publication(client, &args.namespace, config)
        .map_err(std::io::Error::other)?;
    let routes = proxy_with_controller(Some(controller.clone())).await;
    eprintln!("Suntitle WebSocket proxy listening on 0.0.0.0:3030");
    tokio::select! {
        _ = warp::serve(routes).run(([0, 0, 0, 0], 3030)) => {},
        _ = tokio::signal::ctrl_c() => {},
    }
    controller.shutdown().await;
    task.await.map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
async fn proxy() -> BoxedFilter<(impl Reply,)> {
    proxy_with_controller(None).await
}

async fn proxy_with_controller(controller: Option<Controller>) -> BoxedFilter<(impl Reply,)> {
    let state = Transcribers::default();
    let transcribers = warp::any().map(move || state.clone());

    let state = Translators::default();
    let translators = warp::any().map(move || state.clone());

    let state = Displays::default();
    let displays = warp::any().map(move || state.clone());

    let controller = warp::any().map(move || controller.clone());

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
        .and(controller)
        .map(|ws: warp::ws::Ws, controller| {
            ws.on_upgrade(move |socket| registration_connected(socket, controller))
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

async fn registration_connected(ws: WebSocket, controller: Option<Controller>) {
    let Some(controller) = controller else {
        return;
    };
    let (mut write, mut read) = ws.split();
    let Ok(Some(Ok(message))) =
        tokio::time::timeout(std::time::Duration::from_secs(10), read.next()).await
    else {
        return;
    };
    let Ok(envelope) =
        serde_json::from_slice::<Envelope<RegistrationClientMessage>>(message.as_bytes())
    else {
        return;
    };
    if envelope.protocol_version != 1 {
        return;
    }
    let mut connection = match controller.register(envelope.message).await {
        Ok(connection) => connection,
        Err(message) => {
            let _ = write
                .send(Message::text(
                    serde_json::to_string(&Envelope::new(RegistrationServerMessage::Error {
                        message,
                    }))
                    .unwrap(),
                ))
                .await;
            return;
        }
    };
    let registered = RegistrationServerMessage::Registered {
        host: connection.host.clone(),
        session_id: connection.session_id.clone(),
    };
    let result = async {
        write.send(Message::text(serde_json::to_string(&Envelope::new(registered)).unwrap())).await?;
        loop {
            tokio::select! {
                _ = connection.closed.changed() => break,
                outgoing = connection.outbound.recv() => {
                    let Some(outgoing) = outgoing else { break; };
                    tokio::select! {
                        _ = connection.closed.changed() => break,
                        result = tokio::time::timeout(std::time::Duration::from_secs(10), write.send(Message::text(serde_json::to_string(&Envelope::new(outgoing)).unwrap()))) => match result { Ok(result) => result?, Err(_) => break },
                    }
                },
                incoming = read.next() => match incoming {
                    Some(Ok(message)) if message.is_text() => {
                        let Ok(envelope) = serde_json::from_slice::<Envelope<RegistrationClientMessage>>(message.as_bytes()) else { break; };
                        if envelope.protocol_version != 1 { break; }
                        controller.report(&connection, envelope.message).await;
                    },
                    Some(Ok(message)) if message.is_ping() => { write.send(Message::pong(message.as_bytes().to_vec())).await?; },
                    Some(Ok(message)) if !message.is_close() => {},
                    _ => break,
                }
            }
        }
        Ok::<(), warp::Error>(())
    }.await;
    if let Err(e) = result {
        eprintln!("registration socket: {e}");
    }
    controller.disconnect(&connection).await;
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

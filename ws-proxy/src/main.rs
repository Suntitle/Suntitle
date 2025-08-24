// #![deny(warnings)]
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use futures_util::{SinkExt, StreamExt, TryFutureExt};
use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::reply::Reply;
use warp::ws::{Message, WebSocket};

/// Our global unique user id counter.
static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// Our state of currently connected clients.
type Transcribers = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;
type Translators = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;
type Displays = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    let routes = proxy().await;
    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}

async fn proxy() -> BoxedFilter<(impl Reply,)> {
    let state = Transcribers::default();
    let transcribers = warp::any().map(move || state.clone());

    let state = Translators::default();
    let translators = warp::any().map(move || state.clone());

    let state = Displays::default();
    let displays = warp::any().map(move || state.clone());

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

    index.or(transcribe).or(translate).or(display).boxed()
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

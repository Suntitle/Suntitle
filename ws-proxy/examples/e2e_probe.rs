use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const RECEIVE_TIMEOUT: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut audio, _) = connect_async("ws://127.0.0.1:3030/").await?;
    let (mut transcriber, _) = connect_async("ws://127.0.0.1:3030/transcribe").await?;
    let (mut translator, _) = connect_async("ws://127.0.0.1:3030/translate").await?;
    let (mut display, _) = connect_async("ws://127.0.0.1:3030/display").await?;

    let audio_payload = Message::Binary(b"mock-pcm-audio".to_vec().into());
    audio.send(audio_payload.clone()).await?;
    let received_audio = timeout(RECEIVE_TIMEOUT, transcriber.next())
        .await
        .expect("timed out waiting for audio at the transcriber")
        .expect("transcriber connection closed")?;
    assert_eq!(received_audio, audio_payload);

    let transcription = Message::Text("mock transcription".into());
    transcriber.send(transcription.clone()).await?;
    let received_transcription = timeout(RECEIVE_TIMEOUT, translator.next())
        .await
        .expect("timed out waiting for transcription at the translator")
        .expect("translator connection closed")?;
    assert_eq!(received_transcription, transcription);

    let translation = Message::Text("mock translation".into());
    translator.send(translation.clone()).await?;
    let received_translation = timeout(RECEIVE_TIMEOUT, display.next())
        .await
        .expect("timed out waiting for translation at the display")
        .expect("display connection closed")?;
    assert_eq!(received_translation, translation);

    audio.close(None).await?;
    transcriber.close(None).await?;
    translator.close(None).await?;
    display.close(None).await?;

    println!(
        "PASS: live WebSocket relay delivered audio -> transcription -> translation -> display"
    );
    Ok(())
}

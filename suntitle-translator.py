import asyncio
import websockets
import argparse
from transformers import AutoTokenizer, AutoModelForSeq2SeqLM

# Load model directly
tokenizer = AutoTokenizer.from_pretrained("Helsinki-NLP/opus-mt-uk-cs")
model = AutoModelForSeq2SeqLM.from_pretrained("Helsinki-NLP/opus-mt-uk-cs")

def translate_text(text_to_translate):
    input_ids = tokenizer(text_to_translate, return_tensors="pt").input_ids
    outputs = model.generate(input_ids)
    translated_text = tokenizer.decode(outputs[0], skip_special_tokens=True)
    return translated_text

async def websocket_client(host):
    uri = f"ws://{host}/translate"
    reconnect_delay = 1  # seconds
    while True:
        try:
            async with websockets.connect(uri) as websocket:
                print(f"Connected to {uri}")
                sentence_buffer = ""
                while True:
                    try:
                        message = await websocket.recv()
                        print(f"Received: {message}")
                        sentence_buffer += message + " " # Add space to separate words from different messages

                        # Check for sentence-ending punctuation
                        if any(p in sentence_buffer for p in ['.', '!', '?']):
                            # Simple split by sentence-ending punctuation.
                            # This might need more sophisticated NLP for complex cases,
                            # but for subtitles, it should be sufficient.
                            sentences = []
                            last_idx = 0
                            for i, char in enumerate(sentence_buffer):
                                if char in ['.', '!', '?']:
                                    sentences.append(sentence_buffer[last_idx:i+1].strip())
                                    last_idx = i + 1
                            
                            # Keep any incomplete part in the buffer
                            sentence_buffer = sentence_buffer[last_idx:].strip()

                            for sentence in sentences:
                                if sentence: # Ensure it's not an empty string
                                    translated_sentence = translate_text(sentence)
                                    print(f"Translated: {translated_sentence}")
                                    await websocket.send(translated_sentence)
                                    print(f"Sent: {translated_sentence}")

                    except websockets.exceptions.ConnectionClosedOK:
                        print("Connection closed normally. Attempting to reconnect...")
                        break # Break inner loop to trigger reconnection
                    except Exception as e:
                        print(f"An error occurred during message processing: {e}. Attempting to reconnect...")
                        break # Break inner loop to trigger reconnection
        except websockets.exceptions.ConnectionRefused:
            print(f"Connection refused. Retrying in {reconnect_delay} seconds...")
        except Exception as e:
            print(f"Could not connect to {uri}: {e}. Retrying in {reconnect_delay} seconds...")
        
        await asyncio.sleep(reconnect_delay)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Websocket client for subtitle translation.")
    parser.add_argument("--host", default="localhost:3030", help="Websocket server host (e.g., localhost:3030)")
    args = parser.parse_args()

    asyncio.run(websocket_client(args.host))

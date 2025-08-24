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
    async with websockets.connect(uri) as websocket:
        print(f"Connected to {uri}")
        while True:
            try:
                message = await websocket.recv()
                print(f"Received: {message}")
                translated_message = translate_text(message)
                print(f"Translated: {translated_message}")
                await websocket.send(translated_message)
                print(f"Sent: {translated_message}")
            except websockets.exceptions.ConnectionClosedOK:
                print("Connection closed normally.")
                break
            except Exception as e:
                print(f"An error occurred: {e}")
                break

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Websocket client for subtitle translation.")
    parser.add_argument("--host", default="localhost:3030", help="Websocket server host (e.g., localhost:3030)")
    args = parser.parse_args()

    asyncio.run(websocket_client(args.host))

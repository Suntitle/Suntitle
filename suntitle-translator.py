import argparse
import asyncio
from typing import Optional

import websockets


def parse_args(argv: Optional[list[str]] = None):
    parser = argparse.ArgumentParser(description="Websocket client for subtitle translation.")
    parser.add_argument("--host", default="localhost:3030", help="Websocket server authority")
    parser.add_argument("--source-language", required=True)
    parser.add_argument("--target-language", required=True)
    parser.add_argument("--provider", required=True, choices=["huggingface"])
    parser.add_argument("--model", help="Hugging Face model override")
    parser.add_argument("--no-preserve-formatting", action="store_false", dest="preserve_formatting")
    return parser.parse_args(argv)


def model_name(args) -> str:
    return args.model or f"Helsinki-NLP/opus-mt-{args.source_language}-{args.target_language}"


def load_translator(name: str):
    from transformers import AutoModelForSeq2SeqLM, AutoTokenizer

    tokenizer = AutoTokenizer.from_pretrained(name)
    model = AutoModelForSeq2SeqLM.from_pretrained(name)

    def translate(text: str) -> str:
        input_ids = tokenizer(text, return_tensors="pt").input_ids
        output = model.generate(input_ids)
        return tokenizer.decode(output[0], skip_special_tokens=True)

    return translate


def preserve_surrounding_whitespace(source: str, translated: str) -> str:
    leading = source[: len(source) - len(source.lstrip())]
    trailing = source[len(source.rstrip()) :]
    return f"{leading}{translated}{trailing}"


async def websocket_client(args, translate):
    uri = f"ws://{args.host}/translate"
    async with websockets.connect(uri) as websocket:
        print(f"Connected to {uri}")
        async for message in websocket:
            translated = translate(message.strip() if args.preserve_formatting else message)
            if args.preserve_formatting:
                translated = preserve_surrounding_whitespace(message, translated)
            await websocket.send(translated)


def main(argv: Optional[list[str]] = None):
    args = parse_args(argv)
    asyncio.run(websocket_client(args, load_translator(model_name(args))))


if __name__ == "__main__":
    main()

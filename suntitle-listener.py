import argparse
import asyncio
import contextlib
import io
import sys
from typing import Optional, Union

import numpy as np
import sounddevice as sd
import websockets

# --------- Helpers ---------

def list_devices() -> None:
    devices = sd.query_devices()
    default_in = sd.default.device[0]
    print("Input devices:")
    for idx, d in enumerate(devices):
        if d["max_input_channels"] > 0:
            star = " (default)" if idx == default_in else ""
            print(f"[{idx}] {d['name']} — {int(d['default_samplerate'])} Hz max_in_ch={d['max_input_channels']}{star}")

def resolve_input_device(choice: Optional[str]) -> Optional[int]:
    """Return a device index from either an int-like string, None, or a name substring."""
    if choice is None:
        return None  # use PortAudio default
    # Try numeric index first
    try:
        idx = int(choice)
        return idx
    except ValueError:
        pass
    # Otherwise search by substring (case-insensitive) among input-capable devices
    choice_lower = choice.lower()
    devices = sd.query_devices()
    matches = [
        i for i, d in enumerate(devices)
        if d["max_input_channels"] > 0 and choice_lower in d["name"].lower()
    ]
    if not matches:
        raise ValueError(f"No input device matching '{choice}' found.")
    if len(matches) > 1:
        details = ", ".join(f"[{i}] {sd.query_devices(i)['name']}" for i in matches)
        raise ValueError(f"Multiple devices matched '{choice}': {details}. Please specify an index.")
    return matches[0]

def make_wav_header(sample_rate: int, channels: int, dtype=np.int16) -> bytes:
    """Create a standard 44-byte WAV header with unknown data size (0xFFFFFFFF placeholder)."""
    # PCM format
    import struct
    bits_per_sample = np.dtype(dtype).itemsize * 8
    byte_rate = sample_rate * channels * (bits_per_sample // 8)
    block_align = channels * (bits_per_sample // 8)
    # Use 0xFFFFFFFF for data size placeholder; some servers may ignore it.
    data_size = 0xFFFFFFFF
    riff_size = data_size + 36
    return b"".join([
        b"RIFF",
        struct.pack("<I", riff_size),
        b"WAVE",
        b"fmt ",
        struct.pack("<I", 16),                  # Subchunk1Size (PCM)
        struct.pack("<H", 1),                   # AudioFormat (PCM)
        struct.pack("<H", channels),
        struct.pack("<I", sample_rate),
        struct.pack("<I", byte_rate),
        struct.pack("<H", block_align),
        struct.pack("<H", bits_per_sample),
        b"data",
        struct.pack("<I", data_size),
    ])

# --------- Streaming logic ---------

async def stream_audio(
    server_url: str,
    samplerate: int,
    channels: int,
    chunk_ms: int,
    device: Optional[int],
    fmt: str = "raw",
):
    """
    Capture audio from the given device and stream to the WebSocket server as binary frames.
    - fmt="raw": send PCM int16 little-endian chunks
    - fmt="wav": send a single WAV header first, then PCM chunks
    """
    dtype = np.int16
    frames_per_chunk = int(samplerate * (chunk_ms / 1000.0))
    if frames_per_chunk <= 0:
        raise ValueError("chunk_ms too small for given samplerate.")

    # Open audio input stream
    stream = sd.InputStream(
        samplerate=samplerate,
        channels=channels,
        dtype=dtype,
        blocksize=frames_per_chunk,
        device=device,
    )

    print(f"Connecting to {server_url} …")
    async with websockets.connect(server_url, max_size=None, write_limit=2**22) as ws:
        print("Connected.")
        # If WAV mode, send header once (as a binary frame)
        if fmt == "wav":
            header = make_wav_header(samplerate, channels, dtype)
            await ws.send(header)

        # Start audio and pump the queue
        with stream:
            print(
                f"Streaming: {samplerate} Hz | {channels} ch | {chunk_ms} ms chunks | "
                f"device={device if device is not None else 'default'} | format={fmt}"
            )

            try:
                while True:
                    indata, _ = stream.read(frames_per_chunk)
                    await ws.send((indata.astype(dtype, copy=False)).tobytes())  # binary frame
            except asyncio.CancelledError:
                pass

# --------- CLI ---------

def parse_args(argv: Optional[list[str]] = None):
    p = argparse.ArgumentParser(
        description="Stream microphone (or other input) audio to a WebSocket server in fixed-size chunks."
    )
    p.add_argument(
        "server_url",
        nargs="?",
        default="ws://localhost:3030/",
        help="WebSocket server URL, e.g. ws://localhost:3030/ or wss://example.com/prefix/",
    )
    p.add_argument("--chunk-ms", type=int, default=20, help="Chunk size in milliseconds (default: 20)")
    p.add_argument("--samplerate", type=int, default=16000, help="Sample rate in Hz (default: 16000)")
    p.add_argument("--channels", type=int, default=1, help="Number of input channels (default: 1)")
    p.add_argument(
        "--device",
        type=str,
        default=None,
        help="Input device index or name substring (default: system default input)",
    )
    p.add_argument(
        "--format",
        choices=["raw", "wav"],
        default="raw",
        help="Frame format: 'raw' (PCM int16) or 'wav' (header once, then PCM). Default: raw",
    )
    p.add_argument(
        "--list-devices",
        action="store_true",
        help="List available input devices and exit",
    )
    return p.parse_args(argv)

def main():
    args = parse_args()

    if args.list_devices:
        list_devices()
        sys.exit(0)

    try:
        device_index = resolve_input_device(args.device)
    except ValueError as e:
        print(f"Device error: {e}", file=sys.stderr)
        sys.exit(1)

    try:
        asyncio.run(
            stream_audio(
                server_url=args.server_url,
                samplerate=args.samplerate,
                channels=args.channels,
                chunk_ms=args.chunk_ms,
                device=device_index,
                fmt=args.format,
            )
        )
    except KeyboardInterrupt:
        print("\nInterrupted, stopping…")
    except Exception as e:
        print(f"\nError: {e}", file=sys.stderr)
        sys.exit(1)

if __name__ == "__main__":
    main()

#!/usr/bin/env python3
# import AudioProcessor
#import TranscriptionEngine
from whisper_online import *

import sys
import argparse
import os
import logging
import numpy as np
import asyncio
import websockets
import io
import soundfile
import librosa

logger = logging.getLogger(__name__)
parser = argparse.ArgumentParser()

# client options
parser.add_argument("--uri", type=str, default='ws://localhost:3030/transcribe', help="WebSocket URI to connect to")
parser.add_argument("--warmup-file", type=str, dest="warmup_file",
        help="The path to a speech audio wav file to warm up Whisper so that the very first chunk processing is fast. It can be e.g. https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav .")

# options from whisper_online
add_shared_args(parser)
args = parser.parse_args()

# transcription_engine = AudioProcessor.TranscriptionEngine(
#     **vars(args),
# )
set_logging(args,logger,other="")

# setting whisper object by args

SAMPLING_RATE = 16000

size = args.model
language = args.lan
asr, online = asr_factory(args)
min_chunk = args.min_chunk_size

# warm up the ASR because the very first transcribe takes more time than the others.
# Test results in https://github.com/ufal/whisper_streaming/pull/81
msg = "Whisper is not warmed up. The first chunk processing may take longer."
if args.warmup_file:
    if os.path.isfile(args.warmup_file):
        a = load_audio_chunk(args.warmup_file,0,1)
        asr.transcribe(a)
        logger.info("Whisper is warmed up.")
    else:
        logger.critical("The warm up file is not available. "+msg)
        sys.exit(1)
else:
    logger.warning(msg)


class WebSocketProcessor:
    '''
    Wraps WebSocket connection and ASR object, and processes audio chunks.
    '''
    def __init__(self, websocket, online_asr_proc, min_chunk):
        self.websocket = websocket
        self.online_asr_proc = online_asr_proc
        self.min_chunk = min_chunk
        self.last_end = None
        self.is_first = True
        self.audio_buffer = []

    async def receive_audio_chunk(self):
        # Receive all audio that is available by this time
        # Blocks operation if less than self.min_chunk seconds is available
        # Unblocks if connection is closed or a chunk is available
        
        # Accumulate audio until we have enough for a chunk or connection closes
        while sum(len(x) for x in self.audio_buffer) < self.min_chunk * SAMPLING_RATE:
            try:
                raw_bytes = await self.websocket.recv()
                if isinstance(raw_bytes, str): # Handle control messages like "EOS"
                    if raw_bytes == "EOS":
                        logger.info("Received EOS from server.")
                        return None # Signal end of stream
                    else:
                        logger.warning(f"Received unexpected text message: {raw_bytes}")
                        continue

                # Process raw bytes into audio array
                sf = soundfile.SoundFile(io.BytesIO(raw_bytes), channels=1, endian="LITTLE", samplerate=SAMPLING_RATE, subtype="PCM_16", format="RAW")
                audio, _ = librosa.load(sf, sr=SAMPLING_RATE, dtype=np.float32)
                self.audio_buffer.append(audio)
            except websockets.exceptions.ConnectionClosedOK:
                logger.info("Server closed connection normally.")
                return None
            except websockets.exceptions.ConnectionClosedError as e:
                logger.error(f"Server closed connection with error: {e}")
                return None
            except Exception as e:
                logger.error(f"Error receiving audio: {e}")
                return None

        # Extract a chunk from the buffer
        concatenated_audio = np.concatenate(self.audio_buffer)
        chunk_to_process = concatenated_audio[:int(self.min_chunk * SAMPLING_RATE)]
        self.audio_buffer = [concatenated_audio[int(self.min_chunk * SAMPLING_RATE):]] # Keep remaining audio

        if self.is_first and len(chunk_to_process) < self.min_chunk * SAMPLING_RATE:
            return None # Not enough audio for the first chunk
        self.is_first = False
        return chunk_to_process

    def format_output_transcript(self, o, raw):
        if o[0] is not None:
            if raw:
                return f"{o[2]}".lstrip()

            beg, end = o[0]*1000,o[1]*1000
            if self.last_end is not None:
                beg = max(beg, self.last_end)

            self.last_end = end
            print("%1.0f %1.0f %s" % (beg,end,o[2]),flush=True,file=sys.stderr)
            return "%1.0f %1.0f %s" % (beg,end,o[2])
        else:
            logger.debug("No text in this segment")
            return None

    async def send_result(self, o, raw = True):
        msg = self.format_output_transcript(o, raw)
        if msg is not None:
            await self.websocket.send(msg)

    async def handle_websocket_results(self, results_generator):
        """Consumes results from the audio processor and sends them via WebSocket."""
        try:
            async for response in results_generator:
                print(response)
                if response['status'] != 'no_audio_detected':
                    await self.websocket.send(' '.join(response['lines']))
        except websockets.exceptions.ConnectionClosedOK:
            logger.info("WebSocket disconnected while handling results (client likely closed connection).")
        except Exception as e:
            logger.exception(f"Error in WebSocket results handler: {e}")


    async def process(self):
        self.online_asr_proc.init()
        while True:
            a = await self.receive_audio_chunk()
            if a is None: # End of stream or connection closed
                break
            self.online_asr_proc.insert_audio_chunk(a)
            o = online.process_iter()
            try:
                await self.send_result(o)
            except websockets.exceptions.ConnectionClosedOK:
                logger.info("WebSocket connection closed normally during send.")
                break
            except websockets.exceptions.ConnectionClosedError as e:
                logger.error(f"WebSocket connection closed with error during send: {e}")
                break
            except Exception as e:
                logger.error(f"Error sending result: {e}")
                break
        # global transcription_engine
        # audio_processor = AudioProcessor.AudioProcessor(
        #     transcription_engine=transcription_engine,
        # )
        # #await self.websocket.accept()
        # logger.info("WebSocket connection opened.")
                
        # results_generator = await audio_processor.create_tasks()
        # websocket_task = asyncio.create_task(self.handle_websocket_results(results_generator))

        # try:
        #     while True:
        #         message = await self.websocket.recv()
        #         await audio_processor.process_audio(message)
        # except KeyError as e:
        #     if 'bytes' in str(e):
        #         logger.warning(f"Client has closed the connection.")
        #     else:
        #         logger.error(f"Unexpected KeyError in websocket_endpoint: {e}", exc_info=True)
        # except websockets.exceptions.ConnectionClosedError as e:
        #     logger.info("WebSocket disconnected by client during message receiving loop.")
        # except Exception as e:
        #     logger.error(f"Unexpected error in websocket_endpoint main loop: {e}", exc_info=True)
        # finally:
        #     logger.info("Cleaning up WebSocket endpoint...")
        #     if not websocket_task.done():
        #         websocket_task.cancel()
        #     try:
        #         await websocket_task
        #     except asyncio.CancelledError:
        #         logger.info("WebSocket results handler task was cancelled.")
        #     except Exception as e:
        #         logger.warning(f"Exception while awaiting websocket_task completion: {e}")
                
        #     await audio_processor.cleanup()
        #     logger.info("WebSocket endpoint cleaned up successfully.")

async def main():
    reconnect_delay = 1  # seconds
    while True:
        try:
            async with websockets.connect(args.uri) as websocket:
                logger.info(f"Connected to WebSocket server at {args.uri}")
                processor = WebSocketProcessor(websocket, online, args.min_chunk_size)
                await processor.process()
                logger.info("WebSocket client processing finished. Attempting to reconnect...")
                reconnect_delay = 1 # Reset delay on successful connection
        except (websockets.exceptions.ConnectionClosedOK, websockets.exceptions.ConnectionClosedError) as e:
            logger.warning(f"WebSocket connection closed: {e}. Attempting to reconnect in {reconnect_delay} seconds...")
        except ConnectionRefusedError:
            logger.error(f"Connection refused. Is the WebSocket server running at {args.uri}? Retrying in {reconnect_delay} seconds...")
        except Exception as e:
            logger.error(f"An unexpected error occurred: {e}. Retrying in {reconnect_delay} seconds...")
        
        await asyncio.sleep(reconnect_delay)
        reconnect_delay = min(reconnect_delay * 2, 60) # Exponential backoff, max 60 seconds

if __name__ == "__main__":
    asyncio.run(main())

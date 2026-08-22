"""Serve arrow controls that select messages for the Suntitle overlay.

Run this server, open http://localhost:3030, and start the overlay with:
    python suntitle.py --host localhost:3030 --replace-on-receive
"""

import argparse
import asyncio
import json
from http import HTTPStatus

import websockets
from websockets.datastructures import Headers
from websockets.http11 import Response


# Edit this list to choose the subtitles shown by the arrow controls.
MESSAGES = [
    "First subtitle message.",
    "Second subtitle message.",
    "Third subtitle message.",
]


PAGE = """<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Subtitle controls</title>
  <style>
    :root { color-scheme: dark; font-family: system-ui, sans-serif; }
    body {
      min-height: 100vh; margin: 0; display: grid; place-items: center;
      background: #111827; color: #f9fafb;
    }
    main { text-align: center; }
    .controls { display: flex; gap: 2rem; margin-bottom: 1rem; }
    button {
      width: 8rem; height: 6rem; border: 0; border-radius: 1rem;
      background: #2563eb; color: white; font-size: 3rem; cursor: pointer;
    }
    button:hover:not(:disabled) { background: #1d4ed8; }
    button:disabled { opacity: .35; cursor: not-allowed; }
    #position { color: #9ca3af; }
    #message { max-width: 34rem; min-height: 3rem; font-size: 1.25rem; }
  </style>
</head>
<body>
  <main>
    <div class="controls">
      <button id="previous" aria-label="Previous message">&#8592;</button>
      <button id="next" aria-label="Next message">&#8594;</button>
    </div>
    <p id="position">Connecting...</p>
    <p id="message"></p>
  </main>
  <script>
    const previous = document.querySelector('#previous');
    const next = document.querySelector('#next');
    const position = document.querySelector('#position');
    const message = document.querySelector('#message');
    let socket;

    function connect() {
      const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
      socket = new WebSocket(`${protocol}//${location.host}/control`);
      socket.onmessage = ({data}) => {
        const state = JSON.parse(data);
        position.textContent = `${state.index + 1} / ${state.total}`;
        message.textContent = state.message;
        previous.disabled = state.index === 0;
        next.disabled = state.index === state.total - 1;
      };
      socket.onclose = () => {
        position.textContent = 'Disconnected. Reconnecting...';
        previous.disabled = next.disabled = true;
        setTimeout(connect, 1000);
      };
    }

    previous.onclick = () => socket.send('previous');
    next.onclick = () => socket.send('next');
    previous.disabled = next.disabled = true;
    connect();
  </script>
</body>
</html>
"""


class MessageNavigator:
    def __init__(self, messages):
        if not messages:
            raise ValueError("At least one message is required")
        self.messages = messages
        self.index = 0
        self.display_clients = set()
        self.control_clients = set()

    def state(self):
        return json.dumps(
            {
                "index": self.index,
                "total": len(self.messages),
                "message": self.messages[self.index],
            }
        )

    async def broadcast(self, clients, message):
        if clients:
            await asyncio.gather(
                *(client.send(message) for client in clients.copy()),
                return_exceptions=True,
            )

    async def select(self, direction):
        if direction == "previous":
            self.index = max(0, self.index - 1)
        elif direction == "next":
            self.index = min(len(self.messages) - 1, self.index + 1)
        else:
            return

        message = self.messages[self.index]
        print(f"[Server] Selected {self.index + 1}/{len(self.messages)}: {message}")
        await asyncio.gather(
            self.broadcast(self.display_clients, message),
            self.broadcast(self.control_clients, self.state()),
        )

    async def handle(self, websocket):
        path = websocket.request.path
        clients = self.display_clients if path == "/display" else self.control_clients
        clients.add(websocket)
        print(f"[+] {path} client connected: {websocket.remote_address}")

        try:
            if path == "/display":
                # Give a newly opened subtitle overlay the current selection.
                await websocket.send(self.messages[self.index])
                await websocket.wait_closed()
            else:
                await websocket.send(self.state())
                async for command in websocket:
                    await self.select(command)
        except websockets.exceptions.ConnectionClosed:
            pass
        finally:
            clients.discard(websocket)
            print(f"[-] {path} client disconnected: {websocket.remote_address}")


def html_response(body, status=HTTPStatus.OK):
    encoded = body.encode("utf-8")
    headers = Headers(
        {
            "Content-Type": "text/html; charset=utf-8",
            "Content-Length": str(len(encoded)),
        }
    )
    return Response(status.value, status.phrase, headers, encoded)


async def serve(host, port):
    navigator = MessageNavigator(MESSAGES)

    async def process_request(connection, request):
        if request.path == "/":
            return html_response(PAGE)
        if request.path not in ("/control", "/display"):
            return html_response("<h1>404 Not Found</h1>", HTTPStatus.NOT_FOUND)
        return None

    async with websockets.serve(
        navigator.handle,
        host,
        port,
        process_request=process_request,
    ):
        print(f"[Server] Controls: http://{host}:{port}/")
        print(f"[Server] Subtitle WebSocket: ws://{host}:{port}/display")
        await asyncio.get_running_loop().create_future()


def main():
    parser = argparse.ArgumentParser(
        description=(
            "Serve browser arrow controls for suntitle.py. Start the overlay with "
            "--host localhost:3030 --replace-on-receive."
        )
    )
    parser.add_argument("--host", default="localhost", help="Interface to listen on")
    parser.add_argument("--port", type=int, default=3030, help="Port to listen on")
    args = parser.parse_args()

    try:
        asyncio.run(serve(args.host, args.port))
    except KeyboardInterrupt:
        print("\n[Server] Stopped.")


if __name__ == "__main__":
    main()
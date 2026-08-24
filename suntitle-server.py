import asyncio
import websockets

connected_clients = set()

async def handle_client(websocket):
    connected_clients.add(websocket)
    print(f"[+] Client connected: {websocket.remote_address}")

    try:
        async for message in websocket:
            print(f"[Client] {websocket.remote_address}: {message}")
    except websockets.exceptions.ConnectionClosed:
        pass
    finally:
        connected_clients.remove(websocket)
        print(f"[-] Client disconnected: {websocket.remote_address}")

async def send_message_to_clients(message):
    if connected_clients:
        await asyncio.gather(*(client.send(message) for client in connected_clients))
        print(f"[Server] Sent message to {len(connected_clients)} clients.")
    else:
        print("[Server] No clients connected.")

async def cli_input():
    loop = asyncio.get_event_loop()
    while True:
        message = await loop.run_in_executor(None, input, "> ")
        if message.lower() == "exit":
            print("[Server] Exiting input loop.")
            break
        await send_message_to_clients(message)

async def main():
    print("[Server] Starting WebSocket server at ws://localhost:8765")
    server = await websockets.serve(handle_client, "localhost", 8765)

    input_task = asyncio.create_task(cli_input())

    await input_task

    # Graceful shutdown
    server.close()
    await server.wait_closed()
    print("[Server] Shutdown complete.")

if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n[Server] Shutdown via Ctrl+C")

#!/usr/bin/env python3

import logging
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).parent
LOGGER = logging.getLogger(__name__)
SERVER_ADDRESS = ("127.0.0.1", 8000)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.respond()

    def do_POST(self):
        self.respond()

    def do_PUT(self):
        self.respond()

    def respond(self):
        path = unquote(urlsplit(self.path).path).lstrip("/") + ".json"
        file = (ROOT / path).resolve()

        if ROOT.resolve() not in file.parents or not file.is_file():
            self.send_error(404)
            return

        body = file.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    server = ThreadingHTTPServer(SERVER_ADDRESS, Handler)
    try:
        LOGGER.info("Mock server is running at http://%s:%s", *SERVER_ADDRESS)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            LOGGER.info("Mock server shutdown requested")
    finally:
        server.server_close()
        LOGGER.info("Mock server has stopped")


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")
    main()

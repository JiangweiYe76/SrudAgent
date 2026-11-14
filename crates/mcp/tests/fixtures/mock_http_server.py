#!/usr/bin/env python3
"""A stateless Streamable HTTP MCP server: initialize, tools/list, tools/call.

Binds an ephemeral port and prints it on the first line of stdout, so a test
learns where to connect without racing another test for a fixed port. Every
request must carry the bearer token, which is how a test proves the headers an
endpoint was given are actually sent.
"""

import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mock_protocol import answer  # noqa: E402

TOKEN = "Bearer test-token"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):
        pass

    def _empty(self, status):
        self.send_response(status)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def _json(self, payload):
        body = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _authorised(self):
        if self.headers.get("Authorization") == TOKEN:
            return True
        self._empty(401)
        return False

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        message = json.loads(self.rfile.read(length) or b"null")
        if not self._authorised():
            return
        if "id" not in message:
            self._empty(202)
            return
        self._json(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": answer(message.get("method"), message.get("params") or {}),
            }
        )

    def do_GET(self):
        # No server-initiated messages, which the client reads from a 405.
        if self._authorised():
            self._empty(405)

    def do_DELETE(self):
        if self._authorised():
            self._empty(204)


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()

#!/usr/bin/env python3
"""A minimal MCP server over stdio: initialize, tools/list and tools/call."""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mock_protocol import answer  # noqa: E402


def reply(request_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    request_id = message.get("id")
    if request_id is None:
        continue
    reply(
        request_id,
        answer(message.get("method"), message.get("params") or {}),
    )

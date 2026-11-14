"""The messages both mock servers answer, in one place.

Kept apart from the transports so the stdio and HTTP fakes describe the same
server: a test of one is a test of the same tools as the other.
"""

TOOLS = [
    {
        "name": "echo",
        "description": "Echo the text back",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    },
    {
        "name": "add",
        "description": "Add two numbers",
        "inputSchema": {
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
            "required": ["a", "b"],
        },
    },
]


def text(value, is_error=False):
    answer = {"content": [{"type": "text", "text": value}]}
    if is_error:
        answer["isError"] = True
    return answer


def call(name, arguments):
    if name == "echo":
        return text("Echo: " + str(arguments.get("text", "")))
    if name == "add":
        a, b = arguments.get("a"), arguments.get("b")
        if not isinstance(a, (int, float)) or not isinstance(b, (int, float)):
            return text("`a` and `b` must both be numbers", is_error=True)
        if a < 0 or b < 0:
            return text("only non-negative numbers can be added", is_error=True)
        return text(str(a + b))
    return text("no such tool: " + str(name), is_error=True)


def answer(method, params):
    """The result for one JSON-RPC method, or the empty result for the rest."""
    if method == "initialize":
        # Echoed so the handshake succeeds whatever version the client asks for.
        return {
            "protocolVersion": params.get("protocolVersion", "2025-06-18"),
            "capabilities": {"tools": {"listChanged": False}},
            "serverInfo": {"name": "mock", "version": "0"},
            "instructions": "Use the echo tool to repeat text.",
        }
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "tools/call":
        return call(params.get("name"), params.get("arguments") or {})
    return {}

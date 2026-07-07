"""Minimal OpenAI-compatible mock upstream for the appliance demo.

The AOG gateway's `local` provider points here (AOG_LOCAL_BASE). It answers
`POST /v1/chat/completions` with a canned chat.completion + usage, and
`GET /v1/models`, so a governed request through the gateway completes without a
real model server. Not for production — it echoes a fixed message.
"""

import json
from http.server import BaseHTTPRequestHandler, HTTPServer

CANNED = (
    "Hello from the on-prem mock model. Your request was governed by AOG in "
    "shadow mode: classified, routed, metered, and receipted."
)


class Handler(BaseHTTPRequestHandler):
    def _send(self, code: int, obj: object) -> None:
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = self.path.rstrip("/")
        if path == "/v1/models":
            self._send(200, {"object": "list", "data": [{"id": "demo", "object": "model"}]})
        elif path in ("", "/healthz", "/health"):
            self._send(200, {"status": "ok"})
        else:
            self._send(404, {"error": "not found"})

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0) or 0)
        if length:
            self.rfile.read(length)
        if self.path.rstrip("/") == "/v1/chat/completions":
            self._send(
                200,
                {
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "created": 0,
                    "model": "demo",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": CANNED},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 12,
                        "completion_tokens": 20,
                        "total_tokens": 32,
                    },
                },
            )
        else:
            self._send(404, {"error": "not found"})

    def log_message(self, *_args: object) -> None:
        pass


if __name__ == "__main__":
    # Binds to all interfaces by design: this mock runs inside the appliance
    # demo container and must be reachable from the gateway container. Not for
    # production (the module docstring says as much).
    HTTPServer(("0.0.0.0", 8000), Handler).serve_forever()  # noqa: S104

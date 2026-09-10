#!/usr/bin/env python3
"""Drive hexaly-lsp over stdio the way an editor does, and report what it answers.

Kept because it is how the two bugs that the test suite missed were found: a test asserts a
behaviour someone already thought of, this shows what a real session actually produces. Works
locally or over ssh, so the same check runs on a machine with Hexaly and on one without.

    python3 tools/probe.py <file.hxm> [--server /path/to/hexaly-lsp]
"""

import argparse
import json
import pathlib
import subprocess
import sys


class Session:
    def __init__(self, server):
        self.process = subprocess.Popen(
            [server], stdin=subprocess.PIPE, stdout=subprocess.PIPE
        )

    def send(self, message):
        body = json.dumps(message).encode()
        self.process.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.process.stdin.flush()

    def receive(self):
        length = None
        while True:
            line = self.process.stdout.readline()
            if not line:
                return None
            if line == b"\r\n":
                break
            if line.lower().startswith(b"content-length:"):
                length = int(line.split(b":")[1])
        return json.loads(self.process.stdout.read(length))

    def answer(self, request_id, limit=16):
        """The response to one request, skipping notifications that arrive first."""
        for _ in range(limit):
            message = self.receive()
            if message is None:
                return None
            if message.get("id") == request_id:
                return message
            if message.get("method") == "window/logMessage":
                print(f"  log: {message['params']['message']}")
        return None

    def close(self):
        self.process.kill()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("file")
    parser.add_argument("--server", default="hexaly-lsp")
    arguments = parser.parse_args()

    path = pathlib.Path(arguments.file).resolve()
    text = path.read_text(encoding="utf-8")
    uri = path.as_uri()

    session = Session(arguments.server)

    session.send({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"capabilities": {}, "processId": None, "rootUri": path.parent.as_uri()},
    })
    initialize = session.answer(1)
    capabilities = initialize["result"]["capabilities"]
    print("capabilities:", ", ".join(sorted(key for key, value in capabilities.items() if value)))

    session.send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    session.send({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {
            "uri": uri, "languageId": "hexaly", "version": 1, "text": text,
        }},
    })

    # Diagnostics arrive unprompted, so they are read rather than requested.
    for _ in range(8):
        message = session.receive()
        if message is None:
            break
        if message.get("method") == "window/logMessage":
            print(f"  log: {message['params']['message']}")
        if message.get("method") == "textDocument/publishDiagnostics":
            diagnostics = message["params"]["diagnostics"]
            print(f"\n{path.name}: {len(diagnostics)} diagnostic(s)")
            for diagnostic in diagnostics:
                start = diagnostic["range"]["start"]
                print(f"  {start['line'] + 1}:{start['character']} [{diagnostic.get('source')}] {diagnostic['message']}")
            break

    # Completion at the end of the last non-empty line, which is as good a spot as any to confirm
    # the library and the document's own symbols are both offered.
    lines = text.splitlines()
    line = max(index for index, content in enumerate(lines) if content.strip())
    session.send({
        "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
        "params": {"textDocument": {"uri": uri}, "position": {"line": line, "character": 0}},
    })
    response = session.answer(2)
    items = response["result"] if isinstance(response["result"], list) else response["result"]["items"]
    kinds = {}
    for item in items:
        kinds[item.get("kind")] = kinds.get(item.get("kind"), 0) + 1
    print(f"\ncompletion: {len(items)} candidates, kinds {dict(sorted(kinds.items(), key=lambda pair: -pair[1]))}")
    print("  sample:", ", ".join(item["label"] for item in items[:8]))

    session.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())

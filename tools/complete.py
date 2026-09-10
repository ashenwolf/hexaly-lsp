#!/usr/bin/env python3
"""Ask hexaly-lsp for completions after a qualifier, in a real file.

Complements probe.py: that one shows what a whole session produces, this one answers "what do I get
after typing `fn.`" against an actual model, which is the question cross-file resolution exists to
answer.

    python3 tools/complete.py <file.hxm> <qualifier> [--server PATH]
"""

import argparse
import json
import pathlib
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("file")
    parser.add_argument("qualifier", help="what precedes the dot, e.g. fn")
    parser.add_argument("--server", default="hexaly-lsp")
    arguments = parser.parse_args()

    path = pathlib.Path(arguments.file).resolve()
    original = path.read_text(encoding="utf-8")

    # The qualifier is appended in a fresh function rather than edited in place, so the probe cannot
    # depend on where in the file a suitable spot happens to exist.
    probe_line = len(original.splitlines()) + 1
    text = original + f"function __probe() {{\n    {arguments.qualifier}.\n}}\n"
    character = 4 + len(arguments.qualifier) + 1

    process = subprocess.Popen([arguments.server], stdin=subprocess.PIPE, stdout=subprocess.PIPE)

    def send(message):
        body = json.dumps(message).encode()
        process.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        process.stdin.flush()

    def receive():
        length = None
        while True:
            line = process.stdout.readline()
            if not line:
                return None
            if line == b"\r\n":
                break
            if line.lower().startswith(b"content-length:"):
                length = int(line.split(b":")[1])
        return json.loads(process.stdout.read(length))

    root = path.parent
    send({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "capabilities": {}, "processId": None, "rootUri": root.as_uri(),
            "workspaceFolders": [{"uri": root.as_uri(), "name": root.name}],
        },
    })
    while True:
        message = receive()
        if message is None or message.get("id") == 1:
            break

    send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    send({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {
            "uri": path.as_uri(), "languageId": "hexaly", "version": 1, "text": text,
        }},
    })
    send({
        "jsonrpc": "2.0", "id": 2, "method": "textDocument/completion",
        "params": {
            "textDocument": {"uri": path.as_uri()},
            "position": {"line": probe_line, "character": character},
        },
    })

    for _ in range(16):
        message = receive()
        if message is None:
            break
        if message.get("id") == 2:
            result = message["result"]
            items = result if isinstance(result, list) else result["items"]
            print(f"{arguments.qualifier}. in {path.name}: {len(items)} candidate(s)")
            for item in items[:20]:
                print(f"  {item['label']:32} {item.get('detail', '')[:60]}")
            break

    process.kill()
    return 0


if __name__ == "__main__":
    sys.exit(main())

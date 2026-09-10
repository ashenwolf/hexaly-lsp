#!/usr/bin/env python3
"""Print the document outline hexaly-lsp reports for a file.

Nesting is the reason document symbols exist here rather than leaving it to an editor's own outline
query, so this renders the tree indented - a flat result is a visible failure.

    python3 tools/outline.py <file.hxm> [--server PATH] [--search TEXT]

With --search, asks for workspace symbols instead, which needs the file's directory as a root.
"""

import argparse
import json
import pathlib
import subprocess
import sys

KINDS = {
    5: "class", 6: "method", 12: "function", 13: "variable", 2: "module",
    8: "field", 14: "constant",
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("file")
    parser.add_argument("--server", default="hexaly-lsp")
    parser.add_argument("--search", help="query workspace symbols instead of the outline")
    arguments = parser.parse_args()

    path = pathlib.Path(arguments.file).resolve()
    root = path.parent
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

    def answer(request_id):
        for _ in range(16):
            message = receive()
            if message is None or message.get("id") == request_id:
                return message
        return None

    send({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "capabilities": {}, "processId": None, "rootUri": root.as_uri(),
            # Workspace roots are what workspace/symbol searches; the outline does not need them.
            "workspaceFolders": [{"uri": root.as_uri(), "name": root.name}],
        },
    })
    answer(1)
    send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    send({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {
            "uri": path.as_uri(), "languageId": "hexaly", "version": 1,
            "text": path.read_text(encoding="utf-8"),
        }},
    })

    if arguments.search is not None:
        send({"jsonrpc": "2.0", "id": 2, "method": "workspace/symbol",
              "params": {"query": arguments.search}})
        found = answer(2)["result"] or []
        print(f"workspace/symbol {arguments.search!r}: {len(found)} match(es)")
        for symbol in found[:25]:
            container = symbol.get("containerName") or "?"
            print(f"  {KINDS.get(symbol['kind'], symbol['kind']):9} {symbol['name']:34} {container}")
        process.kill()
        return 0

    send({"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol",
          "params": {"textDocument": {"uri": path.as_uri()}}})
    symbols = answer(2)["result"] or []

    def show(items, depth=0):
        for item in items:
            kind = KINDS.get(item["kind"], item["kind"])
            print(f"{'  ' * depth}{kind:9} {item['name']}")
            show(item.get("children") or [], depth + 1)

    print(f"{path.name}: {len(symbols)} top-level symbol(s)")
    show(symbols)
    process.kill()
    return 0


if __name__ == "__main__":
    sys.exit(main())

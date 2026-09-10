#!/usr/bin/env python3
"""Ask hexaly-lsp for signature help on a call, in a real file.

Completes the probe set: probe.py shows a whole session, complete.py answers "what follows this dot",
this one answers "what are this function's parameters". Signature help is the feature most likely to
look present and do nothing, since it only fires inside an unclosed call.

    python3 tools/signature.py <file.hxm> <callee> [--args N] [--server PATH]
"""

import argparse
import json
import pathlib
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("file")
    parser.add_argument("callee", help="the function being called, e.g. defineCapacity or fn.listContains")
    parser.add_argument("--args", type=int, default=0, help="how many arguments are already typed")
    parser.add_argument("--server", default="hexaly-lsp")
    arguments = parser.parse_args()

    path = pathlib.Path(arguments.file).resolve()
    original = path.read_text(encoding="utf-8")

    # The call is appended in a fresh function so the probe does not depend on finding a suitable
    # spot, and left unclosed because that is the only state signature help responds to.
    typed = ", ".join("0" for _ in range(arguments.args))
    separator = ", " if arguments.args else ""
    call = f"    v = {arguments.callee}({typed}{separator}"
    text = original + f"function __probe() {{\n{call}\n}}\n"
    line = len(original.splitlines()) + 1

    process = subprocess.Popen([arguments.server], stdin=subprocess.PIPE, stdout=subprocess.PIPE)

    def send(message):
        body = json.dumps(message).encode()
        process.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        process.stdin.flush()

    def receive():
        length = None
        while True:
            header = process.stdout.readline()
            if not header:
                return None
            if header == b"\r\n":
                break
            if header.lower().startswith(b"content-length:"):
                length = int(header.split(b":")[1])
        return json.loads(process.stdout.read(length))

    def answer(request_id):
        for _ in range(16):
            message = receive()
            if message is None or message.get("id") == request_id:
                return message
        return None

    root = path.parent
    send({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "capabilities": {}, "processId": None, "rootUri": root.as_uri(),
            "workspaceFolders": [{"uri": root.as_uri(), "name": root.name}],
        },
    })
    answer(1)
    send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    send({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {
            "uri": path.as_uri(), "languageId": "hexaly", "version": 1, "text": text,
        }},
    })
    send({
        "jsonrpc": "2.0", "id": 2, "method": "textDocument/signatureHelp",
        "params": {
            "textDocument": {"uri": path.as_uri()},
            "position": {"line": line, "character": len(call)},
        },
    })

    result = answer(2)["result"]
    if not result:
        print(f"{arguments.callee}: no signature help")
        process.kill()
        return 1

    active = result.get("activeParameter")
    print(f"{arguments.callee} in {path.name} (argument {active}):")
    for index, signature in enumerate(result["signatures"]):
        marker = ">" if index == result.get("activeSignature", 0) else " "
        print(f"  {marker} {signature['label']}")
        for position, parameter in enumerate(signature.get("parameters") or []):
            highlight = "*" if position == active else " "
            print(f"      {highlight} {parameter['label']}")

    process.kill()
    return 0


if __name__ == "__main__":
    sys.exit(main())

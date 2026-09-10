#!/usr/bin/env python3
"""Ask hexaly-lsp to format a file, and show what it would change.

Prints a unified diff and never writes to the file: the point is to inspect the edit before trusting
it, since a formatter is the one feature whose failure mode is losing your code.

    python3 tools/format.py <file.hxm> [--mangle] [--server PATH]

--mangle first wrecks the indentation and spacing in memory, which is how you see the formatter do
something on a file that is already correct.
"""

import argparse
import difflib
import json
import pathlib
import re
import subprocess
import sys


def mangle(text):
    """Wreck the layout without changing any token."""
    out = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped:
            out.append("")
            continue
        # Random-ish but deterministic damage: collapse indentation on some lines, over-indent
        # others, and squeeze spaces around operators.
        squeezed = re.sub(r" *(<-|<=|>=|==|\+|,) *", r"\1", stripped)
        out.append(("        " if len(out) % 3 == 0 else "") + squeezed)
    return "\n".join(out) + ("\n" if text.endswith("\n") else "")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("file")
    parser.add_argument("--mangle", action="store_true", help="wreck the layout first, in memory")
    parser.add_argument("--server", default="hexaly-lsp")
    arguments = parser.parse_args()

    path = pathlib.Path(arguments.file).resolve()
    original = path.read_text(encoding="utf-8")
    text = mangle(original) if arguments.mangle else original

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
    initialized = answer(1)
    supported = initialized["result"]["capabilities"].get("documentFormattingProvider")
    print(f"server advertises documentFormattingProvider: {supported}")

    send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    send({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {
            "uri": path.as_uri(), "languageId": "hexaly", "version": 1, "text": text,
        }},
    })
    send({
        "jsonrpc": "2.0", "id": 2, "method": "textDocument/formatting",
        "params": {
            "textDocument": {"uri": path.as_uri()},
            "options": {"tabSize": 4, "insertSpaces": True},
        },
    })

    edits = answer(2)["result"]
    process.kill()

    if not edits:
        print(f"{path.name}: no edits — already formatted" + (" (after mangling?!)" if arguments.mangle else ""))
        return 0

    formatted = edits[0]["newText"]
    diff = difflib.unified_diff(
        text.splitlines(keepends=True), formatted.splitlines(keepends=True),
        fromfile=f"{path.name} (as sent)", tofile=f"{path.name} (formatted)",
    )
    sys.stdout.writelines(diff)

    # The property the server enforces internally, checked here too so the probe can catch a
    # regression the server's own guard somehow let through.
    tokens = lambda s: re.findall(r'"[^"]*"|//[^\n]*|\w+|[^\s\w]', s)
    verdict = "PRESERVED" if tokens(text) == tokens(formatted) else "*** TOKENS CHANGED ***"
    print(f"\ntokens: {verdict}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

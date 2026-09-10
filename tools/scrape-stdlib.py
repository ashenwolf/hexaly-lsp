#!/usr/bin/env python3
"""Extract the Hexaly standard library from a local installation's HTML documentation.

Run this when targeting a new Hexaly version; it writes a JSON artifact that is committed to the
repository and compiled into the binary. The server never scrapes at runtime: the documentation is
part of an installation the user may not have, and its structure is Sphinx output that can change
between releases. Keeping the scrape offline means a doc change breaks this script, visibly, rather
than the language server.

    python3 tools/scrape-stdlib.py /opt/hexaly_14_0 > src/stdlib/hexaly-14.json

The documentation uses a custom Sphinx domain ("hxm"), so entries are `<dl class="hxm KIND">` with
one `<dt class="sig sig-object hxm">` per overload and a `<dd>` holding prose plus a field list.
"""

import html
import json
import pathlib
import re
import sys

# Kinds map onto LSP CompletionItemKind at load time, not here: this file records what Hexaly
# documents, and the LSP vocabulary is the consumer's concern.
KINDS = {
    "globalfunction": "function",
    "globalvariable": "variable",
    "modulefunction": "function",
    "moduleattribute": "constant",
    "class": "class",
    "method": "method",
    "field": "field",
    "operator": "operator",
}

ENTRY = re.compile(r'<dl class="hxm (\w+)">(.*?)(?=<dl class="hxm |</section)', re.S)
SIGNATURE = re.compile(r'<dt class="sig sig-object hxm"[^>]*>(.*?)</dt>', re.S)
ANCHOR = re.compile(r'<dt class="sig sig-object hxm" id="([^"]+)"')
# A parameter is `<strong>name</strong> (<em>Type</em>) - description`, with the type optional. The
# enclosing markup differs by count: several parameters are an `<li><p>` list, a lone one is a bare
# `<p>`, so the wrapper is deliberately not part of the pattern.
PARAMETER = re.compile(
    r"<strong>(\w+)</strong>\s*(?:\(<em>([^<]*)</em>\))?\s*[\u2013\u2014-]\s*(.*?)</p>", re.S
)
PARAMETER_BLOCK = re.compile(
    r'Parameters<span class="colon">:</span></dt>\s*<dd[^>]*>(.*?)</dd>', re.S
)
RETURN_TYPE = re.compile(r'Return type<span class="colon">:</span></dt>\s*<dd[^>]*><p>(.*?)</p>', re.S)


def text(fragment):
    """Visible text of an HTML fragment, with entities resolved and whitespace collapsed."""
    fragment = re.sub(r"<[^>]+>", "", fragment)
    return re.sub(r"\s+", " ", html.unescape(fragment)).strip()


def signature(dt):
    """One rendered signature, e.g. `io.openRead(filename, charset)`.

    The `optional` spans marking variadics (`println([arg0[, arg1[, ...]]])`) are kept as brackets,
    because that is how the documentation conveys "and so on" and dropping them would claim a
    precise arity the function does not have.
    """
    body = re.sub(r'<a class="headerlink".*?</a>', "", dt, flags=re.S)
    body = body.replace('<span class="optional">[</span>', "[").replace('<span class="optional">]</span>', "]")
    return text(body)


def parameters(body):
    """Documented parameters, scoped to this entry's own `Parameters` field.

    Scoped deliberately: entries nest (a class contains its methods), so searching the whole body
    would attribute a method's parameters to the class above it.
    """
    block = PARAMETER_BLOCK.search(body)
    if not block:
        return []

    return [
        {"name": name, "type": text(kind or ""), "doc": text(doc)}
        for name, kind, doc in PARAMETER.findall(block.group(1))
    ]


def documentation(body):
    """The leading prose paragraphs, stopping at the parameter/return field list."""
    prose = body.split('<dl class="field-list', 1)[0]
    paragraphs = [text(paragraph) for paragraph in re.findall(r"<p>(.*?)</p>", prose, re.S)]
    return "\n\n".join(paragraph for paragraph in paragraphs if paragraph)


def entries(path):
    source = path.read_text(encoding="utf-8", errors="ignore")

    for kind, body in ENTRY.findall(source):
        if kind not in KINDS:
            continue

        signatures = [signature(dt) for dt in SIGNATURE.findall(body)]
        if not signatures:
            continue

        # The anchor id is the qualified name Hexaly uses internally (`io.openRead`,
        # `hexaly.HxExpression.isDecision`); the first signature is the display form.
        anchor = ANCHOR.search(body)
        qualified = anchor.group(1) if anchor else signatures[0].split("(")[0]

        # `io.openRead` -> container `io`, name `openRead`. Split on the last dot so the
        # `hexaly.HxExpression.isDecision` form keeps `HxExpression` as its container.
        container, _, name = qualified.rpartition(".")

        returns = RETURN_TYPE.search(body)

        yield {
            "name": name,
            "container": container.rpartition(".")[2],
            "kind": KINDS[kind],
            "signatures": signatures,
            "parameters": parameters(body),
            "returns": text(returns.group(1)) if returns else "",
            "documentation": documentation(body),
        }


def version(root):
    """The installation's version, from the directory name (`hexaly_14_0` -> `14.0`)."""
    name = root.name
    digits = name.removeprefix("hexaly_").removeprefix("localsolver_")
    return digits.replace("_", ".") if digits != name else name


def main():
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <hexaly-install-root>")

    root = pathlib.Path(sys.argv[1])
    library = root / "docs" / "modelerreference" / "standardlibrary"
    if not library.is_dir():
        sys.exit(f"no standard library documentation under {library}")

    symbols = [
        symbol
        for path in sorted(library.rglob("*.html"))
        for symbol in entries(path)
    ]

    # Sorted so the committed artifact has a stable diff between versions, and deduplicated because
    # a symbol documented on an index page as well as its own is one symbol.
    unique = {(symbol["container"], symbol["name"]): symbol for symbol in symbols}
    ordered = [unique[key] for key in sorted(unique)]

    json.dump(
        {"version": version(root), "symbols": ordered},
        sys.stdout,
        indent=1,
        ensure_ascii=False,
    )
    sys.stdout.write("\n")
    print(f"{len(ordered)} symbols from Hexaly {version(root)}", file=sys.stderr)


if __name__ == "__main__":
    main()

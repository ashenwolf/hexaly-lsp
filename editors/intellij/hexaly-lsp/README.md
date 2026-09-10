# Hexaly Language Server

Language support for Hexaly Modeler models (`.hxm`, and the legacy LocalSolver `.lsp`): diagnostics,
completion over 396 standard-library symbols, hover, signature help, go-to-definition, document and
workspace symbols, and formatting.

## Installation

Nothing to install by hand. On first start the installer downloads the binary for this machine from
[GitHub releases](https://github.com/ashenwolf/hexaly-lsp/releases) into
`~/.lsp4ij/lsp/hexaly-lsp` and points the server command at it. Use **Reinstall** to pick up a newer
release.

If you already have `hexaly-lsp` on your `$PATH` — from `cargo install` or a manual download — the
check step finds it and the download is skipped.

## No Hexaly licence is needed

Everything above works with no Hexaly installation at all.

Compiler-backed diagnostics are an additional layer that needs one — they catch duplicate declarations
and unresolvable `use` statements, which parsing alone cannot. That layer never solves, so it **never
consumes a licence seat** and works on a machine whose licence has expired. Without a Hexaly
installation the server says so in the LSP console and continues with everything else.

## What to expect

Diagnostics arrive in two layers: syntax errors on every keystroke, and compiler errors on save. They
are reported under separate sources so they clear independently.

Two things this server deliberately does not do. **Semantic errors inside function bodies** are out of
scope rather than unimplemented: Hexaly's frontend binds declarations without type-checking bodies, so
catching an undefined variable means *running* the model, which needs a licence and executes model
code. And there are **no semantic tokens yet**, which is how JetBrains IDEs colour LSP-backed files —
so expect working intelligence on uncoloured text. Register a TextMate bundle under
**Settings → Editor → TextMate Bundles** for colouring in the meantime.

## Formatting

A conservative normaliser: it fixes indentation and spacing between tokens, and preserves line
structure and blank lines exactly. Every format is verified to tokenize identically to its input before
being offered, so a rule that would rewrite your code returns no edits instead. All 66 models Hexaly
ships come back byte-identical.

To format on save, use **Settings → Tools → Actions on Save → Reformat code**.

# hexaly-lsp

Language server for [Hexaly Modeler](https://www.hexaly.com) models (`.hxm`).

Provides syntax diagnostics from the
[tree-sitter-hexaly](https://github.com/ashenwolf/tree-sitter-hexaly) grammar, compiler-backed
diagnostics from a local Hexaly installation when one is present, and completion, hover and
signature help for the Hexaly standard library.

**No Hexaly licence is required**, and everything except compiler diagnostics works with no Hexaly
installation at all.

## Status

Diagnostics come from two layers, deliberately complementary rather than one falling back to the
other:

| | Syntax (tree-sitter) | Compiler (Hexaly) |
|---|---|---|
| When | every keystroke | on save |
| Errors reported | all of them | one per run |
| Precision | character ranges | line, no column |
| Catches | syntax | duplicate declarations, unresolvable `use` |
| Needs Hexaly | no | yes, but no licence |

Completion, hover and signature help cover **396 standard-library symbols** — with typed
parameters, every documented overload, and the prose from Hexaly's own reference — plus the
declarations in the file being edited. After a dot the candidates narrow to that module or class.

Go-to-definition, document and workspace symbols, and signature help with active-parameter tracking
all work, across module boundaries. Rename and find-references are not planned: they need scope
analysis whose cost is hard to justify for this project.

### Formatting

A *normaliser*, not a pretty-printer, and the distinction is the whole design. A pretty-printer
parses to a tree, discards the layout and re-emits — better output, but any gap in the grammar
becomes mangled code, and this grammar has been wrong three times already. Formatting is the one
feature whose failure mode costs the user their work.

So line structure and blank lines are preserved exactly. Only two things change: each line's leading
whitespace, and the spacing between tokens on it. Where a statement breaks, and which declarations
are grouped, is the author's decision and survives untouched — including Hexaly's own habit of
writing `x_0 <- bool(); x_1 <- bool();` four to a line.

Every format is checked before it is offered: **the output must tokenize identically to the input.**
If a single token differs, the server returns no edits rather than a destructive one. That turns "the
rules are conservative" into a property verified on every keystroke of every file.

The conventions were measured from the 66 models Hexaly ships, not chosen: 4-space indent, same-line
brace, `, ` between arguments, spaced binary operators, tight `...` ranges, and `if (` but `bool()`.
Where the corpus has no convention the formatter has no opinion — multiplicative spacing (`*` is
spaced 70 times against 9 tight; `/` is 12 against 15) and continuation-line indentation (+0 in 191
cases, +8 in 67, +4 in only 13) are left exactly as written.

All 66 official models come back byte-identical. Range formatting is deliberately not offered:
indentation depends on brace depth accumulated from the top of the file, which a selection does not
know.

### Why no licence is needed

Hexaly binds a module's declarations without a licence; the licence is consumed by *solving*. So
the compiler layer runs a load-only invocation and never solves — which also means it never
consumes one of a floating licence's concurrent seats, and works on a machine whose licence has
expired.

The CLI has no parse-only flag, so this is done by writing a small wrapper beside the file:

```
use <module>;
function main() {}
```

`use` binds the target's declarations and the empty `main` then exits. Three properties of this
were measured rather than assumed: the empty `main` is load-bearing (without it Hexaly falls
through to solving), the target's own `model()` body never executes, and exit status distinguishes
a rejected module from a clean one. The wrapper must be a real file in the target's own directory,
because Hexaly resolves `use` against the directory of the script it is given — a temp directory or
a pipe cannot see the target's siblings.

### What is reachable, and what is not

Worth stating up front, because it bounds what this server can ever do. Hexaly's frontend binds
declarations without type-checking function bodies, so it accepts an undefined variable, a type
mismatch, and a wrong-arity builtin call. Catching those means *running* the model, which needs a
licence and executes model code — so **semantic errors inside function bodies are out of scope**,
not merely unimplemented.

A file whose name is not a valid Hexaly identifier (`my-model.hxm`) cannot be loaded as a module,
so it gets syntax diagnostics only.

Completion after a dot needs to know the container's type. That is read from the text rather than
the parse tree, because the case that matters most does not parse: a freshly typed `io.` is an
error node with no member expression in it. A dot after an *expression* (`f().`) is left alone —
resolving it needs type inference this server does not do, and guessing would be worse than
offering the unqualified list.

Signature help tracks the active parameter by counting commas at the right nesting depth, skipping
string literals, and preferring an inner call to an outer one. It reads the text rather than the tree
for the same reason completion does: `computeCost(` has no call node to find yet, and an incomplete
call is exactly when signature help earns its keep.

## Install

Download the archive for your platform from the
[latest release](https://github.com/ashenwolf/hexaly-lsp/releases/latest), unpack it, and put
`hexaly-lsp` somewhere on your `$PATH`:

```sh
tar -xzf hexaly-lsp-aarch64-apple-darwin.tar.gz
mv hexaly-lsp ~/.local/bin/
hexaly-lsp --version
```

Releases cover macOS (arm64, x86_64), Linux (x86_64, arm64) and Windows (x86_64). Each release also
carries `SHA256SUMS`, so a download can be checked with `sha256sum -c SHA256SUMS`.

The Zed extension downloads this automatically — if you use Zed, install the extension and skip this
section entirely.

With a Rust toolchain, `cargo install --git https://github.com/ashenwolf/hexaly-lsp` also works, and
is the right choice if you intend to change the server. Note that it installs to `~/.cargo/bin`,
which is **not** on the `$PATH` a GUI editor inherits on either macOS or Linux — that directory is
added by a shell profile, which only affects processes started from a shell. Editors launched from
Dock, Spotlight or a desktop entry will not find it there unless you configure the path explicitly.

## Editor setup

The server speaks LSP over stdin/stdout and holds no editor-specific code, so any LSP client works.

### Neovim

```lua
vim.filetype.add({ extension = { hxm = "hexaly" } })

vim.lsp.config["hexaly"] = {
  cmd = { "hexaly-lsp" },
  filetypes = { "hexaly" },
  root_markers = { ".git" },
}
vim.lsp.enable("hexaly")
```

### Helix

```toml
# languages.toml
[language-server.hexaly-lsp]
command = "hexaly-lsp"

[[language]]
name = "hexaly"
scope = "source.hxm"
file-types = ["hxm"]
roots = [".git"]
language-servers = ["hexaly-lsp"]
```

### Zed

Install the [zed-hexaly](https://github.com/ashenwolf/zed-hexaly) extension, which carries the
grammar and queries and launches this server. To format on save, add to your settings:

```json
{
  "languages": {
    "Hexaly": { "formatter": "language_server", "format_on_save": "on" }
  }
}
```

### VS Code and Cursor

**Use Hexaly's own extension, not this server.** The official
[Hexaly extension](https://marketplace.visualstudio.com/items?itemName=hexaly.hexaly) is not the
TextMate-grammar wrapper it appears to be: it bundles a complete Hexaly compiler front-end in
JavaScript — lexer, parser, AST, symbol tables — and provides diagnostics, completion, hover,
signature help, definition, references, rename, document highlight, semantic tokens, a formatter and
a debug adapter. That is a superset of this server, in-process and with no Hexaly install needed.

One gap: **Cursor uses Open VSX**, and Hexaly publishes only to the Microsoft Marketplace, so it does
not appear in Cursor's extension search. Download the `.vsix` from the marketplace and use
`Extensions: Install from VSIX`.

This server exists for editors that cannot run that extension — Zed, Neovim, Helix, Emacs — because
neither a TextMate grammar nor a VS Code extension host is available to them.

### Emacs

```elisp
(add-to-list 'auto-mode-alist '("\\.hxm\\'" . hexaly-mode))
(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs '(hexaly-mode . ("hexaly-lsp"))))
```

## Configuration

Hexaly is discovered automatically: `PATH`, then the versioned install directories
(`/opt/hexaly_*`, newest first). To point at a specific installation, pass `hexalyPath` in the
client's `initializationOptions`:

```lua
-- Neovim
vim.lsp.config["hexaly"] = {
  cmd = { "hexaly-lsp" },
  filetypes = { "hexaly" },
  init_options = { hexalyPath = "/opt/hexaly_14_0/bin/hexaly" },
}
```

Discovery is done by the server rather than by each editor extension, so every client needs no
more than a `cmd`. The server logs which installation it found at startup; when there is none it
says so and continues with syntax diagnostics.

## Development

```sh
cargo test          # unit tests plus a stdio session against the built binary
cargo clippy --all-targets
```

The grammar is pinned by `rev` in `Cargo.toml`. A grammar change means landing it in
`tree-sitter-hexaly` and bumping that revision here.

The standard-library artifact (`src/stdlib/hexaly-14.json`) is generated offline and committed:

```sh
python3 tools/scrape-stdlib.py /opt/hexaly_14_0 > src/stdlib/hexaly-14.json
```

It is not scraped at runtime, for two reasons: the documentation belongs to an installation the user
may not have, and it is Sphinx output whose structure can change between releases. Generating it
offline means a documentation change breaks the scraper, visibly, rather than the language server.
The artifact is version-stamped so a mismatch is at least reportable.

Two areas carry most of the risk and are tested directly rather than incidentally:

- **Coordinate conversion.** LSP positions are (line, UTF-16 code unit) pairs; tree-sitter and Rust
  strings are byte-oriented. Every case passes trivially on ASCII, so the tests use Cyrillic text
  (2 bytes per character) and an emoji (4 bytes, 2 UTF-16 units, 1 `char`) to make byte, character
  and UTF-16 counts all differ. A conversion that confuses any pair of them fails.
- **The stdio contract.** `tests/stdio.rs` drives the real binary, parsing `Content-Length` frames
  rather than scanning for JSON, so an unframed byte written to stdout fails the test. stdout
  belongs to the protocol: a stray `println!` corrupts the stream, and log output must go through
  `client.log_message`.

The compiler layer is checked against a local Hexaly installation, including a sweep over every
model Hexaly ships as an example — 65 of the 66 are diagnosable, and none may be reported broken. A
false positive there would be worse than no diagnostics, because a wrong squiggle on correct code
costs more trust than a missing one. Those tests self-skip without an installation, so CI runs
without Hexaly on purpose: the syntax layer has to keep working on such a machine.

## License

MIT — see [LICENSE](LICENSE). Not affiliated with or endorsed by Hexaly.

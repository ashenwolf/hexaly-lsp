# hexaly-lsp

Language server for [Hexaly Modeler](https://www.hexaly.com) models (`.hxm`).

Provides syntax diagnostics from the
[tree-sitter-hexaly](https://github.com/ashenwolf/tree-sitter-hexaly) grammar, plus
compiler-backed diagnostics from a local Hexaly installation when one is present.

**No Hexaly licence is required**, and the syntax layer needs no Hexaly installation at all.

## Status

Two diagnostic layers, deliberately complementary rather than one falling back to the other:

| | Syntax (tree-sitter) | Compiler (Hexaly) |
|---|---|---|
| When | every keystroke | on save |
| Errors reported | all of them | one per run |
| Precision | character ranges | line, no column |
| Catches | syntax | duplicate declarations, unresolvable `use` |
| Needs Hexaly | no | yes, but no licence |

Completion, hover and navigation are planned. None is implemented yet.

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

## Install

```sh
cargo install --git https://github.com/ashenwolf/hexaly-lsp
```

Or build from a clone with `cargo build --release`; the binary lands in
`target/release/hexaly-lsp`.

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
grammar and queries and launches this server.

### VS Code

VS Code ships no generic LSP client, so it needs a client extension — unlike the editors above, it
cannot be pointed at a binary. Either install a generic bridge such as
[glspc](https://marketplace.visualstudio.com/items?itemName=torokati44.glspc) and set the server
path in settings, or write ~20 lines against `vscode-languageclient`.

Note that Hexaly publishes an official VS Code extension with its own TextMate grammar. There this
server adds diagnostics on top of existing highlighting rather than providing it, so a client
extension should contribute only the LSP.

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

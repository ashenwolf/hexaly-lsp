# hexaly-lsp

Language server for [Hexaly Modeler](https://www.hexaly.com) models (`.hxm`).

Provides syntax diagnostics from the
[tree-sitter-hexaly](https://github.com/ashenwolf/tree-sitter-hexaly) grammar. **No Hexaly
installation or licence is required** for what it does today.

## Status

Phase 1: syntax diagnostics only. Every syntax error in the file is reported at once, with real
character ranges, on every keystroke.

Compiler-backed diagnostics (duplicate declarations, unresolvable `use`) and completion are
planned. Neither is implemented yet.

### What is reachable, and what is not

Worth stating up front, because it bounds what this server can ever do. Hexaly's frontend binds
declarations without type-checking function bodies, so `loadModule` accepts an undefined variable,
a type mismatch, and a wrong-arity builtin call. Catching those means *running* the model, which
needs a licence and executes model code — so **semantic errors inside function bodies are out of
scope**, not merely unimplemented.

The compiler is also limited where it does help: it reports one error per parse, with a line and no
column. That is why tree-sitter carries the interactive experience and the compiler layer is
additive rather than primary.

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

## License

MIT — see [LICENSE](LICENSE). Not affiliated with or endorsed by Hexaly.

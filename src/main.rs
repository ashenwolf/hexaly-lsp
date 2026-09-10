use tower_lsp_server::{LspService, Server};

/// Arguments are handled by hand rather than with a parser: there are two, they will not grow, and a
/// dependency for that would be larger than the code it replaced.
///
/// `--version` matters more than it looks. Without it there is no way to tell which build an editor
/// is actually running, which is exactly the question asked whenever behaviour and source disagree.
const USAGE: &str = "\
hexaly-lsp — language server for Hexaly Modeler models (.hxm)

Usage:
  hexaly-lsp            Serve LSP over stdin/stdout. This is how an editor starts it.
  hexaly-lsp --version  Print the version.
  hexaly-lsp --help     Print this message.

Diagnostics from the Hexaly compiler need a Hexaly installation but no licence; the server finds one
on PATH or under /opt/hexaly_*, and works without one.
";

#[tokio::main]
async fn main() {
    // `--version` and `--help` were asked for, so they go to stdout; an unrecognised argument is an
    // error and goes to stderr. Neither path starts the server, so nothing here can corrupt the
    // protocol stream - which is the reason the check happens before the server exists.
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("--help" | "-h") => {
            print!("{USAGE}");
            return;
        }
        Some(unknown) => {
            eprintln!("hexaly-lsp: unrecognised argument {unknown:?}\n\n{USAGE}");
            std::process::exit(2);
        }
        None => {}
    }

    // Editors speak LSP over the process's stdio, which makes stdout unavailable for anything
    // else: a stray `println!` corrupts the message stream. Diagnostic output belongs in
    // `client.log_message`, which the editor surfaces in its language-server log.
    let (service, socket) = LspService::new(hexaly_lsp::server::Backend::new);
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;
}

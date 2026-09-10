use tower_lsp_server::{LspService, Server};

#[tokio::main]
async fn main() {
    // Editors speak LSP over the process's stdio, which makes stdout unavailable for anything
    // else: a stray `println!` corrupts the message stream. Diagnostic output belongs in
    // `client.log_message`, which the editor surfaces in its language-server log.
    let (service, socket) = LspService::new(hexaly_lsp::server::Backend::new);
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;
}

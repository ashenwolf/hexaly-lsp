//! LSP server: document lifecycle and diagnostic publication.

use std::collections::HashMap;

use tokio::sync::Mutex;
use tower_lsp_server::ls_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, InitializeParams,
    InitializeResult, InitializedParams, MessageType, PositionEncodingKind, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use tower_lsp_server::{Client, LanguageServer, jsonrpc};

use crate::diagnostics::syntax;
use crate::document::Document;

/// The parser and the document map live under one lock, taken for the whole of each handler.
///
/// A `tree_sitter::Parser` is not `Sync` and is stateful across calls, so it cannot be shared
/// freely; holding one lock over both also means a document and the parser that produced its tree
/// can never be observed out of step. The critical section is a parse of a single file \u2014
/// sub-millisecond on realistic models \u2014 so contention is not the constraint here.
struct State {
    parser: tree_sitter::Parser,
    documents: HashMap<Uri, Document>,
}

pub struct Backend {
    client: Client,
    state: Mutex<State>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: Mutex::new(State {
                parser: crate::parser(),
                documents: HashMap::new(),
            }),
        }
    }

    /// Publishes with the document version so the client can discard results it has already
    /// superseded. Without it, a slow parse can overwrite the diagnostics of a newer edit.
    async fn publish(&self, uri: Uri) {
        let (diagnostics, version) = {
            let state = self.state.lock().await;
            match state.documents.get(&uri) {
                Some(document) => (syntax::diagnostics(document), document.version()),
                // Closed between the edit and here. `did_close` has already cleared its
                // diagnostics, and republishing now would resurrect them.
                None => return,
            }
        };

        self.client.publish_diagnostics(uri, diagnostics, Some(version)).await;
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                // UTF-16 is the LSP default, stated rather than left implicit because every
                // position this server produces is converted on that assumption.
                position_encoding: Some(PositionEncodingKind::UTF16),
                // Incremental sync is what makes tree-sitter's incremental reparse reachable: a
                // full-text sync would discard the old tree on every keystroke.
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::INCREMENTAL)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            // A non-standard clangd extension, distinct from `position_encoding` above. Declining
            // it keeps this server on the spec.
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "hexaly-lsp ready (syntax diagnostics)")
            .await;
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        {
            let mut state = self.state.lock().await;
            let State { parser, documents } = &mut *state;
            documents.insert(
                document.uri.clone(),
                Document::open(parser, document.text, document.version),
            );
        }

        self.publish(document.uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut state = self.state.lock().await;
            let State { parser, documents } = &mut *state;
            match documents.get_mut(&uri) {
                Some(document) => {
                    document.apply(parser, params.content_changes, params.text_document.version);
                }
                // A change for a document we never opened. Reconstructing it from the changes is
                // only possible for a full replacement, and a client that skips didOpen is
                // misbehaving in ways worth leaving visible rather than papering over.
                None => return,
            }
        }

        self.publish(uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.state.lock().await.documents.remove(&uri);

        // Diagnostics are owned by the server, so a closed file keeps its squiggles in the
        // client's problem list until we retract them.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }
}

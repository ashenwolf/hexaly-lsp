//! LSP server: document lifecycle and diagnostic publication.

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::Mutex;
use tower_lsp_server::ls_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, InitializeParams, InitializeResult, InitializedParams, MessageType,
    PositionEncodingKind, ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use tower_lsp_server::{Client, LanguageServer, jsonrpc};

use crate::diagnostics::{hexaly, syntax};
use crate::discovery::{self, Discovery};
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
    /// Resolved once at `initialize`. Re-resolving per request would let a mid-session PATH change
    /// silently switch compilers, and the answer is not going to change on its own.
    hexaly: Mutex<Discovery>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: Mutex::new(State {
                parser: crate::parser(),
                documents: HashMap::new(),
            }),
            hexaly: Mutex::new(Discovery::Missing),
        }
    }

    /// Publishes with the document version so the client can discard results it has already
    /// superseded. Without it, a slow parse can overwrite the diagnostics of a newer edit.
    async fn publish_syntax(&self, uri: Uri) {
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

    /// Runs the Hexaly frontend over the saved file and publishes what it says.
    ///
    /// Deliberately on save only. The invocation writes a wrapper file into the user's directory,
    /// which is not something to do on every keystroke, and the compiler's one-error-per-run
    /// ceiling makes it a poor fit for live feedback anyway. The syntax layer covers that.
    async fn publish_compiler(&self, uri: Uri) {
        let Some(hexaly) = self.hexaly.lock().await.path().map(PathBuf::from) else {
            return;
        };

        // A URI that is not a local file (a remote or in-memory scheme) cannot be handed to a
        // subprocess.
        let Some(path) = uri.to_file_path() else {
            return;
        };

        let error = hexaly::check(&hexaly, &path).await;

        let diagnostics = {
            let state = self.state.lock().await;
            let Some(document) = state.documents.get(&uri) else {
                return;
            };

            error.map(|error| vec![locate(document, error)]).unwrap_or_default()
        };

        // Published without a version: this reflects the file on disk, not the buffer, so tying it
        // to a buffer version the client may have moved past would have it discarded.
        self.client.publish_diagnostics(uri, diagnostics, None).await;
    }
}

/// Turns a line-granular compiler error into the best range the tree can justify.
///
/// Hexaly gives no column, so the fallback is the whole line. But a module-resolution failure names
/// the module, and the tree knows exactly where that `use` statement's path sits, so those get a
/// real range over the offending name instead of a line-wide smear.
fn locate(document: &Document, error: hexaly::CompilerError) -> Diagnostic {
    let range = hexaly::failed_module(&error.message)
        .and_then(|module| document.module_path_range(module))
        .unwrap_or_else(|| document.line_range(error.line));

    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some(hexaly::SOURCE.to_string()),
        message: error.message,
        ..Diagnostic::default()
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        // Discovery happens here rather than in an editor extension, so every client gets it from
        // a bare `cmd` with no editor-specific glue.
        let configured = params
            .initialization_options
            .as_ref()
            .and_then(|options| options.get("hexalyPath"))
            .and_then(|value| value.as_str())
            .map(PathBuf::from);

        *self.hexaly.lock().await = discovery::locate(configured);

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
        // Reported because "why do I have no compiler diagnostics" is otherwise a question with no
        // visible answer. Absence is normal, so it is informational rather than a warning.
        let discovery = self.hexaly.lock().await.describe();
        self.client.log_message(MessageType::INFO, discovery).await;
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

        self.publish_syntax(document.uri.clone()).await;
        // Diagnose what is on disk at open time too: a file can be saved and broken by another
        // tool, and waiting for the user's first save would show it as clean.
        self.publish_compiler(document.uri).await;
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

        self.publish_syntax(uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.publish_compiler(params.text_document.uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.state.lock().await.documents.remove(&uri);

        // Diagnostics are owned by the server, so a closed file keeps its squiggles in the
        // client's problem list until we retract them.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }
}

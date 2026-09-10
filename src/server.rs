//! LSP server: document lifecycle, diagnostics, and language features.

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::Mutex;
use tower_lsp_server::ls_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, MessageType, OneOf,
    PositionEncodingKind, ServerCapabilities, ServerInfo, SignatureHelp, SignatureHelpOptions, SignatureHelpParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use tower_lsp_server::{Client, LanguageServer, jsonrpc};

use crate::diagnostics::{hexaly, syntax};
use crate::discovery::{self, Discovery};
use crate::document::Document;
use crate::language;
use crate::workspace::Workspace;

/// The parser and the document map live under one lock, taken for the whole of each handler.
///
/// A `tree_sitter::Parser` is not `Sync` and is stateful across calls, so it cannot be shared
/// freely; holding one lock over both also means a document and the parser that produced its tree
/// can never be observed out of step. The critical section is a parse of a single file \u2014
/// sub-millisecond on realistic models \u2014 so contention is not the constraint here.
struct State {
    parser: tree_sitter::Parser,
    documents: HashMap<Uri, Document>,
    /// Modules resolved from disk. Under the same lock as the parser because loading one needs it.
    workspace: Workspace,
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
                workspace: Workspace::default(),
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

        // Workspace roots are where a dotted module path is resolved from. Hexaly resolves `use`
        // relative to the entry point's directory, which the server cannot identify, so the roots
        // are one of the two places searched - the other being the importing file's ancestors.
        let roots: Vec<PathBuf> = params
            .workspace_folders
            .into_iter()
            .flatten()
            .filter_map(|folder| folder.uri.to_file_path().map(|path| path.to_path_buf()))
            .collect();

        if !roots.is_empty() {
            self.state.lock().await.workspace.set_roots(roots);
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                // UTF-16 is the LSP default, stated rather than left implicit because every
                // position this server produces is converted on that assumption.
                position_encoding: Some(PositionEncodingKind::UTF16),
                // Incremental sync is what makes tree-sitter's incremental reparse reachable: a
                // full-text sync would discard the old tree on every keystroke.
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::INCREMENTAL)),
                completion_provider: Some(CompletionOptions {
                    // A dot is the only character that changes what the candidates are, since it
                    // switches the list from globals to one container's members.
                    trigger_characters: Some(vec![".".to_string()]),
                    ..CompletionOptions::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    ..SignatureHelpOptions::default()
                }),
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

        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "{} standard-library symbols loaded (Hexaly {})",
                    crate::stdlib::LIBRARY.symbols.len(),
                    crate::stdlib::LIBRARY.version,
                ),
            )
            .await;
    }

    async fn completion(&self, params: CompletionParams) -> jsonrpc::Result<Option<CompletionResponse>> {
        let position = params.text_document_position;
        let uri = position.text_document.uri;
        let mut state = self.state.lock().await;
        let State {
            parser,
            documents,
            workspace,
        } = &mut *state;

        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let path = uri.to_file_path();
        Ok(Some(CompletionResponse::Array(language::completions(
            document,
            position.position,
            path.as_deref(),
            workspace,
            parser,
        ))))
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let mut state = self.state.lock().await;
        let State {
            parser,
            documents,
            workspace,
        } = &mut *state;

        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let path = uri.to_file_path();
        Ok(
            language::definition(document, position.position, path.as_deref(), workspace, parser)
                .map(GotoDefinitionResponse::Scalar),
        )
    }

    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<Hover>> {
        let position = params.text_document_position_params;
        let state = self.state.lock().await;

        let Some(document) = state.documents.get(&position.text_document.uri) else {
            return Ok(None);
        };

        Ok(language::hover(document, position.position))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> jsonrpc::Result<Option<SignatureHelp>> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let mut state = self.state.lock().await;
        let State {
            parser,
            documents,
            workspace,
        } = &mut *state;

        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let path = uri.to_file_path();
        Ok(language::signature_help(
            document,
            position.position,
            path.as_deref(),
            workspace,
            parser,
        ))
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }

    async fn document_symbol(&self, params: DocumentSymbolParams) -> jsonrpc::Result<Option<DocumentSymbolResponse>> {
        let state = self.state.lock().await;

        let Some(document) = state.documents.get(&params.text_document.uri) else {
            return Ok(None);
        };

        Ok(Some(DocumentSymbolResponse::Nested(language::document_symbols(
            document,
        ))))
    }

    async fn symbol(&self, params: WorkspaceSymbolParams) -> jsonrpc::Result<Option<WorkspaceSymbolResponse>> {
        let mut state = self.state.lock().await;
        let State { parser, workspace, .. } = &mut *state;

        Ok(Some(WorkspaceSymbolResponse::Flat(language::workspace_symbols(
            workspace,
            parser,
            &params.query,
        ))))
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        {
            let mut state = self.state.lock().await;
            let State { parser, documents, .. } = &mut *state;
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
            let State { parser, documents, .. } = &mut *state;
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

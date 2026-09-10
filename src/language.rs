//! Completion, hover and signature help.
//!
//! All three read the same two sources: the standard library artifact and the document's own
//! declarations. Keeping them in one module makes it obvious when they disagree about what the
//! cursor is on, which is the usual way these features drift apart.

use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, Documentation, Hover, HoverContents, MarkupContent, MarkupKind,
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation,
};

use crate::document::Document;
use crate::stdlib::{self, Kind, Symbol};
use crate::symbols::{self, LocalKind};

/// Candidates at `position`.
///
/// After a dot the list is the members of that container and nothing else \u2014 offering globals there
/// would bury the handful of relevant names. Otherwise it is the document's own declarations first,
/// then the library's globals and the container names that can start a qualified call.
pub fn completions(document: &Document, position: Position) -> Vec<CompletionItem> {
    if let Some(qualifier) = document.qualifier_at(position) {
        return stdlib::members(qualifier).map(stdlib_item).collect();
    }

    let locals = symbols::locals(document).into_iter().map(local_item);
    let globals = stdlib::globals().map(stdlib_item);

    // Containers are not symbols in the artifact, so they are synthesised: `io` has to be
    // completable for `io.openRead` to be reachable by typing.
    let mut seen = std::collections::BTreeSet::new();
    let containers = stdlib::containers()
        .filter(|container| seen.insert(*container))
        .map(container_item)
        .collect::<Vec<_>>();

    locals.chain(globals).chain(containers).collect()
}

/// Documentation for the symbol under the cursor.
///
/// A bare name can match several containers (`close` exists on more than one class), and without
/// type inference there is no way to choose. All matches are shown rather than one picked
/// arbitrarily: a reader can tell which applies, a wrong single answer misleads.
pub fn hover(document: &Document, position: Position) -> Option<Hover> {
    let word = document.word_at(position)?;

    let matches: Vec<&Symbol> = match document.qualifier_at(position) {
        Some(qualifier) => stdlib::members(qualifier)
            .filter(|symbol| symbol.name == word)
            .collect(),
        None => stdlib::named(word).collect(),
    };

    if matches.is_empty() {
        return hover_local(document, word);
    }

    let value = matches
        .iter()
        .map(|symbol| symbol.hover())
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: None,
    })
}

/// Hover for something the document declares. Only the declaration itself, since there is no doc
/// comment convention in HXM to read.
fn hover_local(document: &Document, word: &str) -> Option<Hover> {
    let local = symbols::locals(document).into_iter().find(|local| local.name == word)?;

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```hexaly\n{}\n```", local.detail),
        }),
        range: None,
    })
}

/// Signature help for a call at `position`.
///
/// Each documented overload becomes one `SignatureInformation`. The active parameter is not
/// computed: doing it properly means counting commas at the right nesting depth inside a call whose
/// argument list may not parse yet, and a confidently wrong highlight is worse than none.
pub fn signature_help(document: &Document, position: Position) -> Option<SignatureHelp> {
    let word = document.word_at(position)?;

    let symbol = match document.qualifier_at(position) {
        Some(qualifier) => stdlib::members(qualifier).find(|symbol| symbol.name == word),
        None => stdlib::named(word).next(),
    }?;

    let signatures = symbol
        .signatures
        .iter()
        .map(|signature| SignatureInformation {
            label: signature.clone(),
            documentation: (!symbol.documentation.is_empty()).then(|| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: symbol.documentation.clone(),
                })
            }),
            parameters: Some(
                symbol
                    .parameters
                    .iter()
                    .map(|parameter| ParameterInformation {
                        label: ParameterLabel::Simple(parameter.name.clone()),
                        documentation: (!parameter.doc.is_empty()).then(|| {
                            Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: parameter.doc.clone(),
                            })
                        }),
                    })
                    .collect(),
            ),
            active_parameter: None,
        })
        .collect();

    Some(SignatureHelp {
        signatures,
        active_signature: Some(0),
        active_parameter: None,
    })
}

fn stdlib_item(symbol: &Symbol) -> CompletionItem {
    CompletionItem {
        label: symbol.name.clone(),
        kind: Some(completion_kind(symbol.kind)),
        detail: Some(symbol.detail()),
        documentation: (!symbol.documentation.is_empty()).then(|| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: symbol.documentation.clone(),
            })
        }),
        ..CompletionItem::default()
    }
}

fn local_item(local: symbols::Local) -> CompletionItem {
    CompletionItem {
        // Sorted ahead of the library: what the user declared is likelier than any of the stdlib
        // symbols, and clients order by sort_text before label.
        sort_text: Some(format!("0{}", local.name)),
        label: local.name,
        kind: Some(match local.kind {
            LocalKind::Function => CompletionItemKind::FUNCTION,
            LocalKind::Class => CompletionItemKind::CLASS,
            // A decision is the modelling concept this language exists for, and it behaves like a
            // variable at a call site.
            LocalKind::Decision | LocalKind::Variable => CompletionItemKind::VARIABLE,
            LocalKind::Module => CompletionItemKind::MODULE,
        }),
        detail: Some(local.detail),
        ..CompletionItem::default()
    }
}

fn container_item(container: &str) -> CompletionItem {
    CompletionItem {
        label: container.to_string(),
        kind: Some(CompletionItemKind::MODULE),
        ..CompletionItem::default()
    }
}

/// Kinds are mapped rather than all reported as `KEYWORD`, because the icon a client draws is the
/// fastest signal in a completion list about what a name will do.
fn completion_kind(kind: Kind) -> CompletionItemKind {
    match kind {
        Kind::Function => CompletionItemKind::FUNCTION,
        Kind::Method | Kind::Operator => CompletionItemKind::METHOD,
        Kind::Variable => CompletionItemKind::VARIABLE,
        Kind::Constant => CompletionItemKind::CONSTANT,
        Kind::Field => CompletionItemKind::FIELD,
        Kind::Class => CompletionItemKind::CLASS,
    }
}

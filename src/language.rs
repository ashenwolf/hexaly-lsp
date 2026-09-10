//! Completion, hover and signature help.
//!
//! All three read the same two sources: the standard library artifact and the document's own
//! declarations. Keeping them in one module makes it obvious when they disagree about what the
//! cursor is on, which is the usual way these features drift apart.

use std::path::Path;

use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, Documentation, Hover, HoverContents, Location, MarkupContent, MarkupKind,
    ParameterInformation, ParameterLabel, Position, Range, SignatureHelp, SignatureInformation, Uri,
};

use crate::document::Document;
use crate::stdlib::{self, Kind, Symbol};
use crate::symbols::{self, LocalKind};
use crate::workspace::Workspace;

/// Candidates at `position`.
///
/// After a dot the list is the members of that container and nothing else \u2014 offering globals there
/// would bury the handful of relevant names. Otherwise it is the document's own declarations first,
/// then the library's globals and the container names that can start a qualified call.
/// The language's own keywords, from the reference's keyword list.
///
/// Not in the standard-library artifact, and correctly so: `constraint` and `minimize` are grammar,
/// not functions. But they are what a modeller types most, so completion that omitted them would
/// feel broken in a way no amount of library coverage compensates for.
///
/// `pragma` and the reserved-for-future-use words are deliberately absent: suggesting a word that
/// does nothing yet is worse than not suggesting it.
const KEYWORDS: &[&str] = &[
    "break",
    "catch",
    "class",
    "constraint",
    "constructor",
    "continue",
    "do",
    "else",
    "false",
    "final",
    "for",
    "function",
    "if",
    "in",
    "inf",
    "is",
    "local",
    "maximize",
    "minimize",
    "nan",
    "new",
    "nil",
    "override",
    "return",
    "static",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "use",
    "while",
    "with",
];

/// Candidates at `position`.
///
/// After a dot the list is that container's members and nothing else, since offering globals there
/// would bury the handful of relevant names. Resolution is tried in order of specificity: a module
/// alias in this file, then a class declared in this file, then the standard library. That order
/// matters because the project's own names shadow nothing but are what the user reaches for most —
/// in a real model, `fn.listContains` and `inputs.numIntervals` outnumber every stdlib call.
///
/// `workspace` and `parser` are needed only for the cross-file case, and threaded rather than held
/// because the server owns both.
pub fn completions(
    document: &Document,
    position: Position,
    path: Option<&Path>,
    workspace: &mut Workspace,
    parser: &mut tree_sitter::Parser,
) -> Vec<CompletionItem> {
    if let Some(qualifier) = document.qualifier_at(position) {
        return qualified(document, qualifier, path, workspace, parser);
    }

    let locals = symbols::locals(document).into_iter().map(local_item);
    let keywords = KEYWORDS.iter().map(|keyword| keyword_item(keyword));
    let globals = stdlib::globals().map(stdlib_item);

    // Containers are not symbols in the artifact, so they are synthesised: `io` has to be
    // completable for `io.openRead` to be reachable by typing.
    let mut seen = std::collections::BTreeSet::new();
    let containers = stdlib::containers()
        .filter(|container| seen.insert(*container))
        .map(container_item)
        .collect::<Vec<_>>();

    locals.chain(keywords).chain(globals).chain(containers).collect()
}

fn qualified(
    document: &Document,
    qualifier: &str,
    path: Option<&Path>,
    workspace: &mut Workspace,
    parser: &mut tree_sitter::Parser,
) -> Vec<CompletionItem> {
    // A `use` statement in this file, in either of its two forms. This is the case that matters most
    // on a real model and needs no type inference, only name resolution.
    let binding = symbols::locals(document).into_iter().find(|local| {
        matches!(local.kind, LocalKind::Module | LocalKind::Import)
            && local.name == qualifier
            && local.module_path.is_some()
    });

    if let (Some(local), Some(path)) = (binding.as_ref(), path) {
        let module_path = local.module_path.as_deref().unwrap_or_default();

        let resolved = match local.kind {
            // `use Name from module;` binds one member, so what is wanted is that class's members,
            // one level deeper than the module's own surface.
            LocalKind::Import => workspace.imported_members(parser, path, module_path, qualifier),
            _ => workspace
                .exports(parser, path, module_path)
                .map(|exports| exports.to_vec()),
        };

        if let Some(members) = resolved {
            return members.into_iter().map(local_item).collect();
        }
    }

    // A class declared in this file. `SpecialLocations.PICK_MANUAL` is reached far more often in a
    // real model than any library class.
    let class = symbols::class_members(document, qualifier);
    if !class.is_empty() {
        return class.into_iter().map(local_item).collect();
    }

    stdlib::members(qualifier).map(stdlib_item).collect()
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

/// Where a name is defined.
///
/// Three cases, in the order they are tried: a declaration in this file, the file a `use` statement
/// names, and a member of a module reached through an alias. The last is what makes
/// `fn.listContains` navigable, and it reuses the module resolution completion already needs — no
/// separate index.
///
/// Returns `None` for a stdlib symbol: hover already shows its documentation, and there is no source
/// file to jump to.
pub fn definition(
    document: &Document,
    position: Position,
    path: Option<&Path>,
    workspace: &mut Workspace,
    parser: &mut tree_sitter::Parser,
) -> Option<Location> {
    let word = document.word_at(position)?;
    let path = path?;

    // A qualified reference: resolve the binding, then find the member inside that file.
    if let Some(qualifier) = document.qualifier_at(position) {
        let binding = symbols::locals(document)
            .into_iter()
            .find(|local| matches!(local.kind, LocalKind::Module | LocalKind::Import) && local.name == qualifier)?;

        let module_path = binding.module_path.as_deref()?;
        let target = workspace.resolve(path, module_path)?;

        // Both forms land in the same file; the member is looked up by name either way, since a
        // class member and a module-level declaration are both declarations in that file.
        return member_location(parser, &target, word);
    }

    // An unqualified name declared in this file.
    if let Some(range) = document.declaration_range(word) {
        return Some(Location {
            uri: uri_for(path)?,
            range,
        });
    }

    // The name in a `use` statement: jump to the module it names, or to the member it imports.
    let binding = symbols::locals(document)
        .into_iter()
        .find(|local| matches!(local.kind, LocalKind::Module | LocalKind::Import) && local.name == word)?;

    let target = workspace.resolve(path, binding.module_path.as_deref()?)?;

    // An imported member has a declaration to land on; a whole module does not, so the file itself
    // is the answer there.
    let imported = (binding.kind == LocalKind::Import)
        .then(|| member_location(parser, &target, word))
        .flatten();

    if let Some(location) = imported {
        return Some(location);
    }

    Some(Location {
        uri: uri_for(&target)?,
        range: Range::default(),
    })
}

/// The location of `name` inside an already-resolved module file.
fn member_location(parser: &mut tree_sitter::Parser, target: &Path, name: &str) -> Option<Location> {
    let text = std::fs::read_to_string(target).ok()?;
    let module = Document::open(parser, text, 0);

    Some(Location {
        uri: uri_for(target)?,
        range: module.declaration_range(name)?,
    })
}

/// A `file:` URI for a path. Built by hand because `Uri` here is a plain string type with no path
/// conversion of its own, and the server only ever deals in absolute paths.
fn uri_for(path: &Path) -> Option<Uri> {
    format!("file://{}", path.to_str()?).parse().ok()
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
            // An imported member is usually a class in practice, and MODULE would draw the wrong
            // icon for something that is not one.
            LocalKind::Import => CompletionItemKind::CLASS,
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

fn keyword_item(keyword: &str) -> CompletionItem {
    CompletionItem {
        label: keyword.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        // Between the document's own names and the library: a keyword is likelier than any given
        // stdlib symbol, but less likely than something the user just declared.
        sort_text: Some(format!("1{keyword}")),
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

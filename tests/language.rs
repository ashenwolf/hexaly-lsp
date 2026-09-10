//! Tests for completion, hover and signature help.
//!
//! These exercise the cursor-context logic as much as the data: a completion list is only useful if
//! the server correctly decides *where* the cursor is, and that decision is made against a tree
//! whose call may not parse yet.

use hexaly_lsp::document::Document;
use hexaly_lsp::{language, stdlib, symbols};
use tower_lsp_server::ls_types::{CompletionItemKind, Documentation, HoverContents, Position};

fn open(text: &str) -> Document {
    let mut parser = hexaly_lsp::parser();
    Document::open(&mut parser, text.to_string(), 1)
}

fn at(line: u32, character: u32) -> Position {
    Position { line, character }
}

#[test]
fn the_bundled_library_loaded() {
    // A guard on the artifact itself: `include_str!` cannot fail at runtime, but a scrape that
    // silently produced almost nothing would pass every other test in this file.
    assert!(
        stdlib::LIBRARY.symbols.len() > 300,
        "only {} symbols",
        stdlib::LIBRARY.symbols.len()
    );
    assert_eq!(stdlib::LIBRARY.version, "14.0");
}

#[test]
fn documented_overloads_and_types_survived_the_scrape() {
    let open_read = stdlib::members("io")
        .find(|symbol| symbol.name == "openRead")
        .expect("io.openRead is documented");

    // Two overloads, the second taking a charset. This is the case the whole artifact exists for:
    // a static keyword list could not express it.
    assert_eq!(open_read.signatures.len(), 2);
    assert!(open_read.signatures[1].contains("charset"));
    assert_eq!(open_read.returns, "StreamReader");
    assert_eq!(open_read.parameters.len(), 2);
    assert_eq!(open_read.parameters[0].r#type, "String");
    assert!(open_read.documentation.contains("reading mode"));
}

#[test]
fn variadic_signatures_keep_their_brackets() {
    let println = stdlib::globals()
        .find(|symbol| symbol.name == "println")
        .expect("println is documented");

    // `println([arg0[, arg1[, ...]]])` - rewriting this as `println(arg0, arg1)` would claim an
    // arity the function does not have.
    assert!(println.signatures[0].contains('['), "got {:?}", println.signatures);
    assert!(println.signatures[0].contains("..."), "got {:?}", println.signatures);
}

#[test]
fn a_dot_narrows_completion_to_that_container() {
    let document = open("use io;\nfunction main() {\n    io.\n}\n");
    let items = language::completions(&document, at(2, 7));

    assert!(!items.is_empty(), "no candidates after `io.`");
    // Members only. Offering 396 globals here would bury the handful that apply.
    assert!(
        items.iter().any(|item| item.label == "openRead"),
        "io members missing: {:?}",
        items.iter().take(5).map(|item| &item.label).collect::<Vec<_>>()
    );
    assert!(
        !items.iter().any(|item| item.label == "println"),
        "globals leaked into a qualified completion"
    );
}

#[test]
fn unqualified_completion_offers_locals_globals_and_containers() {
    let document = open("use io;\nfunction model() {\n    nbItems = 5;\n    x[i in 0...nbItems] <- bool();\n    \n}\n");
    let items = language::completions(&document, at(4, 4));

    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();

    assert!(labels.contains(&"model"), "own function missing");
    assert!(labels.contains(&"x"), "own decision missing");
    assert!(labels.contains(&"println"), "stdlib global missing");
    assert!(
        labels.contains(&"io"),
        "container missing, so `io.` is unreachable by typing"
    );
}

#[test]
fn own_declarations_sort_before_the_library() {
    let document = open("function model() {\n    chosen[i in 0...3] <- bool();\n    \n}\n");
    let items = language::completions(&document, at(2, 4));

    let own = items.iter().find(|item| item.label == "chosen").expect("own decision");
    let library = items
        .iter()
        .find(|item| item.label == "println")
        .expect("stdlib global");

    // Clients order by sort_text before label, so a prefix is how a local outranks 396 others.
    assert!(
        own.sort_text.is_some(),
        "a local needs a sort key to outrank the library"
    );
    assert!(
        own.sort_text < library.sort_text.clone().or(Some(library.label.clone())),
        "own={:?} library={:?}",
        own.sort_text,
        library.sort_text,
    );
}

#[test]
fn completion_kinds_are_varied_not_all_keywords() {
    let document = open("function model() {\n    \n}\n");
    let items = language::completions(&document, at(1, 4));

    let kinds: Vec<CompletionItemKind> = items.iter().filter_map(|item| item.kind).collect();
    let distinct = kinds
        .iter()
        .fold(Vec::new(), |mut unique: Vec<CompletionItemKind>, kind| {
            if !unique.contains(kind) {
                unique.push(*kind);
            }
            unique
        });

    // The icon a client draws is the fastest signal about what a name does, so reporting everything
    // as KEYWORD would throw away information the artifact already has.
    assert!(distinct.len() > 2, "kinds not differentiated: {distinct:?}");
    assert!(kinds.contains(&CompletionItemKind::FUNCTION));
}

#[test]
fn a_dot_narrows_completion_mid_word_too() {
    // The commoner case than a bare dot: the user has typed part of the member name. `io.openR` does
    // not parse as a member expression either, so this also has to come from the text.
    let document = open("use io;\nfunction main() {\n    reader = io.openR\n}\n");
    let items = language::completions(&document, at(2, 21));

    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    assert!(labels.contains(&"openRead"), "got {labels:?}");
    // Filtering the prefix is the client's job; the server's is to offer the right container.
    assert!(!labels.contains(&"println"), "globals leaked: {labels:?}");
}

#[test]
fn a_dot_after_an_expression_is_not_treated_as_a_container() {
    // `f().` and `x[0].` are method calls on a value whose type this server cannot infer, so there is
    // no honest candidate list. Guessing one would be worse than offering globals.
    let document = open("function main() {\n    y = f().\n}\n");
    let items = language::completions(&document, at(1, 12));

    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    assert!(labels.contains(&"println"), "expected the unqualified list: {labels:?}");
}

#[test]
fn hover_documents_a_library_symbol() {
    let document = open("function main() {\n    reader = io.openRead(\"data.txt\");\n}\n");

    let hover = language::hover(&document, at(1, 17)).expect("hover on openRead");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markdown");
    };

    assert!(markup.value.contains("io.openRead(filename)"), "got {}", markup.value);
    assert!(markup.value.contains("reading mode"), "prose missing");
    assert!(markup.value.contains("StreamReader"), "return type missing");
    assert!(markup.value.contains("`filename`"), "parameters missing");
}

#[test]
fn hover_falls_back_to_the_documents_own_declaration() {
    let document = open("function model() {\n    capacity[i in 0...3] <- int(0, 10);\n    y = capacity;\n}\n");

    let hover = language::hover(&document, at(2, 9)).expect("hover on capacity");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markdown");
    };

    // Nothing in the library is called `capacity`, so this can only come from the tree.
    assert!(markup.value.contains("capacity"), "got {}", markup.value);
    assert!(
        markup.value.contains("int(0, 10)"),
        "declaration not shown: {}",
        markup.value
    );
}

#[test]
fn hover_on_nothing_is_none() {
    let document = open("function model() {\n    x <- bool();\n}\n");
    assert!(language::hover(&document, at(0, 0)).is_none());
}

#[test]
fn signature_help_lists_every_overload() {
    let document = open("function main() {\n    reader = io.openRead(\n}\n");

    let help = language::signature_help(&document, at(1, 17)).expect("signature help for openRead");

    assert_eq!(help.signatures.len(), 2, "both overloads expected");
    assert!(help.signatures[0].label.contains("openRead"));

    let parameters = help.signatures[1].parameters.as_ref().expect("parameters");
    assert_eq!(parameters.len(), 2);
    assert!(matches!(
        help.signatures[0].documentation,
        Some(Documentation::MarkupContent(_))
    ));

    // Deliberately absent: computing it means counting commas at the right nesting depth in a call
    // that may not parse, and a confidently wrong highlight is worse than none.
    assert!(help.active_parameter.is_none());
}

#[test]
fn locals_cover_the_declaration_forms_that_matter() {
    let document = open(
        "use io;\nuse utils.helpers as helpers;\nclass Item { field weight; }\nfunction model() {\n    \
         local total = 0;\n    x[i in 0...3] <- bool();\n}\n",
    );

    let found = symbols::locals(&document);
    let names: Vec<&str> = found.iter().map(|local| local.name.as_str()).collect();

    assert!(names.contains(&"model"), "function missing: {names:?}");
    assert!(names.contains(&"Item"), "class missing: {names:?}");
    assert!(names.contains(&"x"), "indexed decision missing: {names:?}");
    assert!(names.contains(&"io"), "module missing: {names:?}");
    // An alias is the name the user will actually type, so it wins over the module path.
    assert!(names.contains(&"helpers"), "module alias missing: {names:?}");
}

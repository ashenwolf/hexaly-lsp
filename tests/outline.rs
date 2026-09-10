//! Tests for document symbols and workspace-wide symbol search.
//!
//! Nesting is the reason document symbols exist here at all - a flat list is what the editor's own
//! outline query already produces - so these check structure rather than mere presence.

use std::path::PathBuf;

use hexaly_lsp::document::Document;
use hexaly_lsp::workspace::Workspace;
use hexaly_lsp::{language, symbols};
use tower_lsp_server::ls_types::SymbolKind;

fn open(text: &str) -> Document {
    let mut parser = hexaly_lsp::parser();
    Document::open(&mut parser, text.to_string(), 1)
}

const MODEL: &str = "\
use io;
class Item {
    field weight;
    constructor(w) { this.weight = w; }
    describe() { return weight; }
}
function model() {
    chosen[i in 0...3] <- bool();
    local scratch = 0;
    constraint chosen[0] <= 1;
    maximize sum[i in 0...3](chosen[i]);
}
function helper(a) { return a; }
";

#[test]
fn a_class_contains_its_members() {
    // The case a flat outline gets wrong: these would be four siblings of the class rather than its
    // children.
    let document = open(MODEL);
    let outline = language::document_symbols(&document);

    let item = outline.iter().find(|symbol| symbol.name == "Item").expect("class Item");
    assert_eq!(item.kind, SymbolKind::CLASS);

    let children = item.children.as_ref().expect("a class with members has children");
    let names: Vec<&str> = children.iter().map(|child| child.name.as_str()).collect();

    assert!(names.contains(&"weight"), "field missing: {names:?}");
    assert!(names.contains(&"constructor"), "constructor missing: {names:?}");
    assert!(names.contains(&"describe"), "method missing: {names:?}");
}

#[test]
fn a_function_contains_its_decisions_and_objectives() {
    // In the production model this targets, every decision lives inside a function, so a flat list of
    // decisions has no structure to show and the function is the unit a reader navigates to.
    let document = open(MODEL);
    let outline = language::document_symbols(&document);

    let model = outline
        .iter()
        .find(|symbol| symbol.name == "model")
        .expect("function model");
    assert_eq!(model.kind, SymbolKind::FUNCTION);

    let children = model.children.as_ref().expect("model declares things");
    let names: Vec<&str> = children.iter().map(|child| child.name.as_str()).collect();

    assert!(names.contains(&"chosen"), "decision missing: {names:?}");
    // `maximize` has no name of its own, so it is labelled by its direction: it is the point of a
    // model and a reader looks for it.
    assert!(names.contains(&"maximize"), "objective missing: {names:?}");

    // A plain local inside a body is noise in an outline, however useful it is in completion.
    assert!(
        !names.contains(&"scratch"),
        "a local leaked into the outline: {names:?}"
    );
}

#[test]
fn use_statements_are_not_outline_entries() {
    // They are bindings, not structure. Listing them buries the declarations the outline was opened
    // to find.
    let document = open(MODEL);
    let outline = language::document_symbols(&document);
    let names: Vec<&str> = outline.iter().map(|symbol| symbol.name.as_str()).collect();

    assert!(
        !names.contains(&"io"),
        "a use statement appeared in the outline: {names:?}"
    );
    assert_eq!(names, vec!["Item", "model", "helper"], "top level, in source order");
}

#[test]
fn selection_range_covers_the_name_not_the_body() {
    // The distinction the LSP draws: `range` selects the declaration, `selection_range` places the
    // cursor. Reversing them selects a whole function body on a click.
    let document = open(MODEL);
    let outline = language::document_symbols(&document);

    let model = outline.iter().find(|symbol| symbol.name == "model").unwrap();

    assert_eq!(model.selection_range.start.line, model.selection_range.end.line);
    assert!(
        model.range.end.line > model.range.start.line,
        "the declaration spans its body"
    );
    assert!(model.range.start.line <= model.selection_range.start.line);
}

#[test]
fn outline_is_empty_for_a_file_with_no_declarations() {
    let document = open("// just a comment\n");
    assert!(language::document_symbols(&document).is_empty());
}

/// A small tree, since workspace search is about finding things across files.
fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("hexaly-lsp-symbols-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("utils")).expect("temp dir");
    std::fs::create_dir_all(root.join("build")).expect("temp dir");

    std::fs::write(root.join("model.hxm"), "function model() { x <- bool(); }\n").expect("write");
    std::fs::write(
        root.join("utils/helpers.hxm"),
        "function listContains(list, item) { return false; }\nclass Helper { field a; }\n",
    )
    .expect("write");
    // Build output must not be searched: it is where the file count explodes and holds nothing a
    // user navigates to.
    std::fs::write(
        root.join("build/generated.hxm"),
        "function shouldNotAppear() { return 1; }\n",
    )
    .expect("write");

    root
}

#[test]
fn workspace_search_finds_declarations_across_files() {
    // The capability an editor's outline query cannot provide at all: reaching a declaration in one
    // of a dozen files without knowing which.
    let root = fixture("search");
    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    workspace.set_roots(vec![root.clone()]);

    let found = language::workspace_symbols(&mut workspace, &mut parser, "list");
    let names: Vec<&str> = found.iter().map(|symbol| symbol.name.as_str()).collect();

    assert!(names.contains(&"listContains"), "got {names:?}");
    assert!(
        found
            .iter()
            .any(|symbol| symbol.location.uri.as_str().ends_with("utils/helpers.hxm")),
        "the match should point at the file that declares it"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn workspace_search_is_case_insensitive_and_matches_substrings() {
    let root = fixture("case");
    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    workspace.set_roots(vec![root.clone()]);

    // What an editor's symbol prompt expects: typing part of a name, in any case, finds it.
    let found = language::workspace_symbols(&mut workspace, &mut parser, "CONTAINS");
    assert!(
        found.iter().any(|symbol| symbol.name == "listContains"),
        "case-insensitive substring match expected"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn workspace_search_skips_build_output() {
    let root = fixture("skip");
    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    workspace.set_roots(vec![root.clone()]);

    let found = language::workspace_symbols(&mut workspace, &mut parser, "");
    let names: Vec<&str> = found.iter().map(|symbol| symbol.name.as_str()).collect();

    assert!(names.contains(&"model"), "own model missing: {names:?}");
    assert!(
        !names.contains(&"shouldNotAppear"),
        "build output was searched: {names:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn workspace_search_names_the_file_as_the_container() {
    // So a prompt showing two `close` functions tells the user which module each belongs to.
    let root = fixture("container");
    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    workspace.set_roots(vec![root.clone()]);

    let found = language::workspace_symbols(&mut workspace, &mut parser, "listContains");
    let symbol = found.first().expect("one match");

    assert_eq!(symbol.container_name.as_deref(), Some("helpers"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn outline_kinds_are_differentiated() {
    let document = open(MODEL);
    let outline = language::document_symbols(&document);

    let kinds: Vec<SymbolKind> = outline.iter().map(|symbol| symbol.kind).collect();
    assert!(kinds.contains(&SymbolKind::CLASS));
    assert!(kinds.contains(&SymbolKind::FUNCTION));

    // A decision maps to VARIABLE - the closest LSP kind - with the detail line carrying what it
    // actually is.
    let model = outline.iter().find(|symbol| symbol.name == "model").unwrap();
    let chosen = model
        .children
        .as_ref()
        .unwrap()
        .iter()
        .find(|child| child.name == "chosen")
        .unwrap();
    assert_eq!(chosen.kind, SymbolKind::VARIABLE);
    assert!(
        chosen.detail.as_deref().is_some_and(|detail| detail.contains("<-")),
        "the detail should show it is a decision: {:?}",
        chosen.detail
    );
}

#[test]
fn outline_covers_the_same_ground_as_the_editor_query() {
    // The Zed extension ships an outline.scm listing functions, classes, methods, constructors,
    // indexed declarations and objectives. This must not regress against it, since the LSP takes
    // precedence when both are present.
    let document = open(MODEL);
    let outline = symbols::outline(&document);

    let mut everything: Vec<String> = Vec::new();
    let mut stack: Vec<&symbols::Outline> = outline.iter().collect();
    while let Some(item) = stack.pop() {
        everything.push(item.name.clone());
        stack.extend(item.children.iter());
    }

    for expected in [
        "Item",
        "model",
        "helper",
        "weight",
        "constructor",
        "describe",
        "chosen",
        "maximize",
    ] {
        assert!(
            everything.contains(&expected.to_string()),
            "{expected} missing from {everything:?}"
        );
    }
}

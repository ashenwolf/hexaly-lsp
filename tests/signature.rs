//! Tests for signature help.
//!
//! Split out because this covers the case the feature was originally missing entirely: a call to the
//! project's own function, in a call that does not parse yet. An unclosed `computeCost(` has no
//! `call_expression` node at all - the bare paren is an error node - so nothing tree-based can find
//! it, and an unclosed call is the only time signature help is wanted.

use std::path::PathBuf;

use hexaly_lsp::document::Document;
use hexaly_lsp::language;
use hexaly_lsp::workspace::Workspace;
use tower_lsp_server::ls_types::{ParameterLabel, Position, SignatureHelp};

fn open(text: &str) -> Document {
    let mut parser = hexaly_lsp::parser();
    Document::open(&mut parser, text.to_string(), 1)
}

/// Signature help at the position just after the given text, which is where an editor asks from.
fn help_after(document: &Document, needle: &str) -> Option<SignatureHelp> {
    let (line, text) = document
        .text()
        .lines()
        .enumerate()
        .find(|(_, text)| text.contains(needle))?;

    let column = text.find(needle).unwrap() + needle.len();
    let position = Position {
        line: line as u32,
        character: column as u32,
    };

    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    language::signature_help(document, position, None, &mut workspace, &mut parser)
}

fn labels(help: &SignatureHelp) -> Vec<String> {
    help.signatures[0]
        .parameters
        .as_ref()
        .map(|parameters| {
            parameters
                .iter()
                .map(|parameter| match &parameter.label {
                    ParameterLabel::Simple(name) => name.clone(),
                    ParameterLabel::LabelOffsets(_) => String::new(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn describes_a_call_to_the_documents_own_function() {
    // The gap that prompted this. In a real model the hard-to-remember parameter list belongs to a
    // project function like `defineCapacity(modelInputs, modelConfig, throughput, ...)`, and no
    // amount of standard-library coverage helps with it.
    let document = open(
        "function computeCost(weight, distance, factor) { return weight; }\n\
         function model() {\n    x = computeCost(\n}\n",
    );

    let help = help_after(&document, "computeCost(").expect("own function described");

    assert!(
        help.signatures[0].label.contains("computeCost"),
        "got {:?}",
        help.signatures[0].label
    );
    assert_eq!(labels(&help), vec!["weight", "distance", "factor"]);
}

#[test]
fn tracks_which_argument_the_cursor_is_in() {
    let document = open(
        "function computeCost(weight, distance, factor) { return weight; }\n\
         function model() {\n    x = computeCost(1, 2, \n}\n",
    );

    let help = help_after(&document, "computeCost(1, 2, ").expect("described");

    // Two commas before the cursor, so the third parameter is active.
    assert_eq!(help.active_parameter, Some(2));
}

#[test]
fn an_inner_call_wins_over_an_outer_one() {
    let document = open(
        "function outer(a) { return a; }\n\
         function inner(x, y) { return x; }\n\
         function model() {\n    v = outer(inner(\n}\n",
    );

    let help = help_after(&document, "outer(inner(").expect("described");

    // The cursor is inside `inner`, so describing `outer` would be actively misleading.
    assert!(
        help.signatures[0].label.contains("inner"),
        "got {:?}",
        help.signatures[0].label
    );
    assert_eq!(labels(&help), vec!["x", "y"]);
}

#[test]
fn a_closed_inner_call_does_not_capture_the_cursor() {
    let document = open(
        "function outer(a, b) { return a; }\n\
         function inner(x) { return x; }\n\
         function model() {\n    v = outer(inner(1), \n}\n",
    );

    let help = help_after(&document, "outer(inner(1), ").expect("described");

    // `inner(1)` is balanced, so the cursor belongs to `outer` - and its comma must count as one of
    // outer's, not one of inner's.
    assert!(
        help.signatures[0].label.contains("outer"),
        "got {:?}",
        help.signatures[0].label
    );
    assert_eq!(help.active_parameter, Some(1));
}

#[test]
fn a_comma_inside_a_string_is_not_an_argument_separator() {
    let document = open(
        "function label(text, width) { return text; }\n\
         function model() {\n    v = label(\"a, b, c\", \n}\n",
    );

    let help = help_after(&document, "label(\"a, b, c\", ").expect("described");

    // Three commas precede the cursor but two are inside the literal; counting them would highlight
    // a parameter that does not exist.
    assert_eq!(help.active_parameter, Some(1));
}

#[test]
fn a_call_spanning_several_lines_is_still_described() {
    // Real Hexaly calls wrap. A scan that stopped at a newline would fail exactly where signature
    // help is most useful.
    let document = open(
        "function wide(a, b, c) { return a; }\n\
         function model() {\n    v = wide(\n        1,\n        2,\n        \n}\n",
    );

    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    let position = Position { line: 5, character: 8 };

    let help = language::signature_help(&document, position, None, &mut workspace, &mut parser)
        .expect("a multi-line call is still a call");

    assert!(help.signatures[0].label.contains("wide"));
    assert_eq!(help.active_parameter, Some(2));
}

#[test]
fn a_statement_boundary_ends_the_search() {
    let document = open("function f(a) { return a; }\nfunction model() {\n    x = 1;\n    \n}\n");

    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    let position = Position { line: 3, character: 4 };

    // Nothing is being called here, and answering with the previous statement's call would be noise.
    assert!(language::signature_help(&document, position, None, &mut workspace, &mut parser).is_none());
}

#[test]
fn an_index_space_is_not_a_call() {
    let document = open("function model() {\n    x[i in 0...3] <- bool();\n}\n");

    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    let position = Position { line: 1, character: 7 };

    // `x[` is a subscript or an index space; there is no signature to describe for one.
    assert!(language::signature_help(&document, position, None, &mut workspace, &mut parser).is_none());
}

#[test]
fn the_documents_own_function_wins_over_a_library_symbol() {
    // A model may well define its own `min`. The one that would actually be called is the local one.
    let document = open(
        "function min(first, second, third) { return first; }\n\
         function model() {\n    v = min(\n}\n",
    );

    let help = help_after(&document, "min(").expect("described");

    assert_eq!(
        labels(&help),
        vec!["first", "second", "third"],
        "the local declaration should win"
    );
}

#[test]
fn describes_a_function_reached_through_a_module_alias() {
    let root = std::env::temp_dir().join(format!("hexaly-lsp-sig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("utils")).expect("temp dir");
    std::fs::write(
        root.join("utils/helpers.hxm"),
        "function listContains(array, item) { return false; }\n",
    )
    .expect("write");

    let path: PathBuf = root.join("model.hxm");
    let source = "use utils.helpers as fn;\nfunction model() {\n    v = fn.listContains(\n}\n";
    std::fs::write(&path, source).expect("write");

    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    let document = Document::open(&mut parser, source.to_string(), 1);

    let line = source.lines().nth(2).unwrap();
    let position = Position {
        line: 2,
        character: (line.find('(').unwrap() + 1) as u32,
    };

    let help = language::signature_help(&document, position, Some(&path), &mut workspace, &mut parser)
        .expect("a cross-file function is described");

    assert!(
        help.signatures[0].label.contains("listContains"),
        "got {:?}",
        help.signatures[0].label
    );
    assert_eq!(labels(&help), vec!["array", "item"]);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn selects_the_overload_matching_the_argument_count() {
    // `io.openRead` has a one- and a two-parameter form. Typing a second argument should move the
    // highlight to the form that has one.
    let document = open("function main() {\n    r = io.openRead(\"f.txt\", \n}\n");

    let help = help_after(&document, "io.openRead(\"f.txt\", ").expect("described");

    assert_eq!(help.active_parameter, Some(1));
    assert_eq!(
        help.active_signature,
        Some(1),
        "the two-parameter overload covers a second argument"
    );
}

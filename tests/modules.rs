//! Tests for cross-file module resolution, local classes as containers, and go-to-definition.
//!
//! The fixture mirrors the shape of the production model this was built against - a nested tree of
//! modules imported under aliases - because that shape is what drove the design. Flat single-file
//! tests would pass while the feature that matters stayed broken.

use std::path::{Path, PathBuf};

use hexaly_lsp::document::Document;
use hexaly_lsp::workspace::Workspace;
use hexaly_lsp::{language, symbols};
use tower_lsp_server::ls_types::Position;

/// A tree shaped like the real model: aliased imports, a nested directory, a class of constants.
fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("hexaly-lsp-modules-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("utils")).expect("temp dir");
    std::fs::create_dir_all(root.join("inputs")).expect("temp dir");

    // `use` must precede every declaration in HXM - a misplaced one is a syntax error that breaks
    // the parse of everything after it - so the fixture is ordered as real code has to be. It
    // imports both ways, since neither may be re-exported.
    std::fs::write(
        root.join("utils/functionalUtils.hxm"),
        "use io;\n\
         use Imported from inputs.specialLocations;\n\
         local moduleWide = 7;\n\
         function listContains(list, item) {\n    local counter = 0;\n    return false;\n}\n\
         function listMap(list, f) { return list; }\n\
         function filter(list, predicate) { return list; }\n",
    )
    .expect("write");

    std::fs::write(
        root.join("inputs/specialLocations.hxm"),
        "class SpecialLocations {\n    static PICK_MANUAL = \"pick\";\n    static REBIN = \"rebin\";\n\
         \n    describe(which) { return which; }\n}\n",
    )
    .expect("write");

    std::fs::write(
        root.join("model.hxm"),
        "use utils.functionalUtils as fn;\n\
         use inputs.specialLocations as locations;\n\
         use SpecialLocations from inputs.specialLocations;\n\
         class Local { field weight; }\n\
         function model() {\n    chosen[i in 0...3] <- bool();\n}\n",
    )
    .expect("write");

    root
}

fn open(path: &Path) -> (tree_sitter::Parser, Document) {
    let mut parser = hexaly_lsp::parser();
    let text = std::fs::read_to_string(path).expect("fixture exists");
    let document = Document::open(&mut parser, text, 1);
    (parser, document)
}

/// The position just after `qualifier.` in a document, located by searching the text.
///
/// Hand-counted line and column numbers have broken this suite three times: once wrong from the
/// start, twice made stale by editing the fixture. Deriving them means a fixture change cannot
/// silently move a cursor onto the wrong token.
fn after_dot(document: &Document, qualifier: &str) -> Position {
    let needle = format!("{qualifier}.");
    let (line, text) = document
        .text()
        .lines()
        .enumerate()
        .find(|(_, text)| text.contains(&needle))
        .expect("the probe line is in the document");

    let column = text.find(&needle).expect("just matched") + needle.len();
    Position {
        line: line as u32,
        character: column as u32,
    }
}

fn at(line: u32, character: u32) -> Position {
    Position { line, character }
}

#[test]
fn a_dotted_module_path_resolves_to_a_nested_file() {
    let root = fixture("resolve");
    let workspace = Workspace::default();

    let resolved = workspace
        .resolve(&root.join("model.hxm"), "utils.functionalUtils")
        .expect("utils.functionalUtils resolves");

    assert_eq!(resolved, root.join("utils/functionalUtils.hxm"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn resolution_is_entry_point_relative_not_file_relative() {
    // Verified against the compiler before being encoded here: a file at `sub/deep.hxm` saying
    // `use helpers;` fails even when `utils/helpers.hxm` exists, and `use utils.helpers;` from that
    // same nested file succeeds. So the path is joined onto a root, never onto the importer's own
    // directory.
    let root = fixture("relative");
    let workspace = Workspace::default();
    let importer = root.join("inputs/specialLocations.hxm");

    // From a nested file, the dotted path still resolves from the root above it.
    assert_eq!(
        workspace.resolve(&importer, "utils.functionalUtils"),
        Some(root.join("utils/functionalUtils.hxm")),
    );

    // A bare name is not resolved against the importer's own directory.
    assert_eq!(workspace.resolve(&importer, "functionalUtils"), None);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completion_after_a_module_alias_lists_that_modules_functions() {
    // The case that motivated this phase. In the production model, `fn.listContains` and
    // `fn.filter` outnumber every standard-library call, and none of it needs type inference.
    let root = fixture("alias");
    let path = root.join("model.hxm");
    let (mut parser, document) = open(&path);
    let mut workspace = Workspace::default();

    // Cursor just after `fn.` appended to the file.
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("function extra() {\n    fn.\n}\n");
    let document = {
        let _ = document;
        Document::open(&mut parser, text, 2)
    };

    let items = language::completions(
        &document,
        after_dot(&document, "fn"),
        Some(&path),
        &mut workspace,
        &mut parser,
    );
    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();

    assert!(labels.contains(&"listContains"), "got {labels:?}");
    assert!(labels.contains(&"filter"), "got {labels:?}");
    assert!(labels.contains(&"listMap"), "got {labels:?}");

    // A top-level `local` IS part of the module's surface.
    assert!(labels.contains(&"moduleWide"), "top-level local missing: {labels:?}");

    // A variable inside one of the module's functions is NOT reachable as `fn.counter`, so offering
    // it would be a suggestion that cannot compile. Found on the real model, where `fn.` listed loop
    // counters alongside the functions.
    assert!(
        !labels.contains(&"counter"),
        "a variable inside a function body leaked into the module surface: {labels:?}"
    );

    // Neither `use` form is re-exported. A module it imports (`use io;`) and a member it imports
    // (`use SpecialLocations from ...;`) are both unreachable through the importing alias, and the
    // second was found leaking on the real model once the import form started resolving.
    assert!(
        !labels.contains(&"io"),
        "a module's imports must not be re-exported: {labels:?}"
    );
    assert!(
        !labels.contains(&"Imported"),
        "an imported member must not be re-exported: {labels:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completion_after_a_local_class_lists_its_members() {
    // `SpecialLocations.PICK_MANUAL` is reached more often in the real model than any library class.
    let root = fixture("class");
    let path = root.join("inputs/specialLocations.hxm");
    let (mut parser, _) = open(&path);
    let mut workspace = Workspace::default();

    let text = std::fs::read_to_string(&path).unwrap() + "function use_it() {\n    SpecialLocations.\n}\n";
    let document = Document::open(&mut parser, text, 2);

    let items = language::completions(
        &document,
        after_dot(&document, "SpecialLocations"),
        Some(&path),
        &mut workspace,
        &mut parser,
    );
    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();

    assert!(labels.contains(&"PICK_MANUAL"), "static field missing: {labels:?}");
    assert!(labels.contains(&"REBIN"), "static field missing: {labels:?}");
    assert!(labels.contains(&"describe"), "method missing: {labels:?}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completion_after_an_imported_member_lists_that_classs_members() {
    // `use SpecialLocations from inputs.specialLocations;` binds one member, not the module, so
    // `SpecialLocations.` must reach one level deeper than a module alias would. Found on the real
    // model, where this form returned nothing at all.
    let root = fixture("importform");
    let path = root.join("model.hxm");
    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();

    let text = std::fs::read_to_string(&path).unwrap() + "function use_it() {\n    SpecialLocations.\n}\n";
    let document = Document::open(&mut parser, text, 2);

    let items = language::completions(
        &document,
        after_dot(&document, "SpecialLocations"),
        Some(&path),
        &mut workspace,
        &mut parser,
    );
    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();

    assert!(labels.contains(&"PICK_MANUAL"), "got {labels:?}");
    assert!(labels.contains(&"describe"), "got {labels:?}");
}

#[test]
fn definition_finds_a_declaration_in_the_same_file() {
    let root = fixture("localdef");
    let path = root.join("model.hxm");
    let (mut parser, _) = open(&path);
    let mut workspace = Workspace::default();

    let text = std::fs::read_to_string(&path).unwrap() + "function other() {\n    model();\n}\n";
    let document = Document::open(&mut parser, text, 2);

    let call = document
        .text()
        .lines()
        .position(|line| line.contains("model();"))
        .expect("the call is in the document") as u32;
    let location = language::definition(&document, at(call, 6), Some(&path), &mut workspace, &mut parser)
        .expect("model() is declared in this file");

    assert!(
        location.uri.as_str().ends_with("model.hxm"),
        "got {}",
        location.uri.as_str()
    );
    // Derived, not hard-coded: the fixture gained a line and this assert would otherwise be stale.
    let declared = document
        .text()
        .lines()
        .position(|line| line.contains("function model()"))
        .expect("model is declared in the fixture") as u32;
    assert_eq!(location.range.start.line, declared);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn definition_crosses_a_use_boundary() {
    // `fn.listContains` jumps into utils/functionalUtils.hxm, at the function itself rather than the
    // top of the file. This reuses the same resolution completion needs, so there is no second index
    // to keep in step.
    let root = fixture("crossdef");
    let path = root.join("model.hxm");
    let (mut parser, _) = open(&path);
    let mut workspace = Workspace::default();

    let text = std::fs::read_to_string(&path).unwrap() + "function other() {\n    fn.listContains(1, 2);\n}\n";
    let document = Document::open(&mut parser, text, 2);

    let cursor = after_dot(&document, "fn");
    let location = language::definition(
        &document,
        Position {
            line: cursor.line,
            character: cursor.character + 3,
        },
        Some(&path),
        &mut workspace,
        &mut parser,
    )
    .expect("fn.listContains resolves across the use boundary");

    assert!(
        location.uri.as_str().ends_with("utils/functionalUtils.hxm"),
        "got {}",
        location.uri.as_str()
    );

    // The line is derived from the fixture rather than hard-coded, so reordering it cannot make this
    // assert a stale number - which is exactly what happened when `use io;` moved to the top.
    let target = std::fs::read_to_string(root.join("utils/functionalUtils.hxm")).unwrap();
    let expected = target
        .lines()
        .position(|line| line.contains("function listContains"))
        .expect("listContains is declared in the fixture") as u32;
    assert_eq!(location.range.start.line, expected);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn definition_on_an_alias_jumps_to_the_module() {
    let root = fixture("aliasdef");
    let path = root.join("model.hxm");
    let (mut parser, document) = open(&path);
    let mut workspace = Workspace::default();

    // The `fn` in `use utils.functionalUtils as fn;`. The column is computed rather than counted:
    // hand-counted positions have been wrong twice in this suite.
    let first_line = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    let column = first_line.rfind("fn").expect("the alias is on the first line") as u32;

    let location = language::definition(&document, at(0, column), Some(&path), &mut workspace, &mut parser)
        .expect("the alias names a module");

    assert!(
        location.uri.as_str().ends_with("utils/functionalUtils.hxm"),
        "got {}",
        location.uri.as_str()
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_use_statement_records_both_its_alias_and_its_path() {
    // Both are needed and they differ: the alias is what appears before a dot in code, the path is
    // what resolves to a file.
    let root = fixture("aliaspath");
    let (_parser, document) = open(&root.join("model.hxm"));

    let locals = symbols::locals(&document);
    let alias = locals.iter().find(|local| local.name == "fn").expect("alias `fn`");

    assert_eq!(alias.module_path.as_deref(), Some("utils.functionalUtils"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unresolvable_alias_falls_back_rather_than_erroring() {
    let root = fixture("missing");
    let mut parser = hexaly_lsp::parser();
    let mut workspace = Workspace::default();
    let path = root.join("model.hxm");

    let document = Document::open(
        &mut parser,
        "use nowhere.missing as gone;\nfunction f() {\n    gone.\n}\n".to_string(),
        1,
    );

    // No panic, no error: an unresolvable module yields an empty list, and the editor shows nothing
    // rather than the wrong thing.
    let items = language::completions(
        &document,
        after_dot(&document, "gone"),
        Some(&path),
        &mut workspace,
        &mut parser,
    );
    assert!(items.is_empty(), "expected no candidates, got {}", items.len());

    let _ = std::fs::remove_dir_all(&root);
}

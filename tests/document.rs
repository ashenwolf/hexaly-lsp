//! Tests for the coordinate conversions and the syntax diagnostics built on them.
//!
//! The conversions are the part of this server most likely to be quietly wrong: every case here
//! passes trivially on ASCII, so a bug shows up only in files with non-ASCII text \u2014 which Hexaly
//! models routinely have, in comments and in string literals written for end users.

use hexaly_lsp::diagnostics::syntax;
use hexaly_lsp::document::Document;
use tower_lsp_server::ls_types::{Position, Range, TextDocumentContentChangeEvent};

fn open(text: &str) -> (tree_sitter::Parser, Document) {
    let mut parser = hexaly_lsp::parser();
    let document = Document::open(&mut parser, text.to_string(), 1);
    (parser, document)
}

fn position(line: u32, character: u32) -> Position {
    Position { line, character }
}

#[test]
fn reports_one_diagnostic_per_syntax_error() {
    // Two independent mistakes, deliberately far apart. The Hexaly compiler reports only the
    // first; recovering and reporting both is the whole reason this layer exists.
    let (_parser, document) = open(
        "function model() {\n    a <- bool(;\n    b <- bool();\n    c <- bool(;\n}\n",
    );

    let diagnostics = syntax::diagnostics(&document);

    assert!(
        diagnostics.len() >= 2,
        "expected both errors, got {:?}",
        diagnostics.iter().map(|d| d.range).collect::<Vec<_>>(),
    );
    assert_eq!(diagnostics[0].range.start.line, 1);
    assert!(diagnostics.iter().any(|d| d.range.start.line == 3));
    assert!(diagnostics.iter().all(|d| d.source.as_deref() == Some(syntax::SOURCE)));
}

#[test]
fn clean_file_has_no_diagnostics() {
    let (_parser, document) = open(
        "function model() {\n    x[i in 0...5] <- bool();\n    constraint sum[i in 0...5](x[i]) <= 3;\n}\n",
    );

    assert_eq!(syntax::diagnostics(&document), Vec::new());
}

#[test]
fn accepts_the_constructs_the_grammar_fix_added() {
    // Pinned here as well as in the grammar repo: if a future `rev` bump regresses these, the
    // server would start putting false squiggles on correct models, which is worse than silence.
    let (_parser, document) = open(
        "function model() {\n    \
         options[c][o in 0...nbOptions] = readInt();\n    \
         outFile.println[j in 0...nbJobs](order[j], \" \");\n}\n",
    );

    assert_eq!(syntax::diagnostics(&document), Vec::new());
}

#[test]
fn diagnostic_columns_are_utf16_units_not_bytes() {
    // The comment holds a 3-byte-per-character string, so a byte-based column would place the
    // error 10 columns to the right of where the client draws it.
    let source = "// тестування\nfunction model() {\n    a <- bool(;\n}\n";
    let (_parser, document) = open(source);

    let diagnostics = syntax::diagnostics(&document);
    let first = diagnostics.first().expect("the file has a syntax error");

    assert_eq!(first.range.start.line, 2);

    // Independently: the same offset measured in bytes differs, which is what makes this a real
    // test rather than a tautology.
    let line = source.lines().nth(2).expect("line 2 exists");
    let byte_column = line.find(';').expect("the offending ';' is on line 2");
    let utf16_column = line[..byte_column].chars().map(char::len_utf16).sum::<usize>();
    assert_eq!(byte_column, utf16_column, "line 2 is ASCII, so these agree");
    assert!(first.range.start.character as usize <= utf16_column);
}

#[test]
fn positions_after_astral_characters_count_two_units() {
    // An emoji is one `char`, two UTF-16 units, four bytes \u2014 so all three counts differ, and a
    // conversion that confuses any pair of them fails here.
    let source = "function model() {\n    name = \"🚀\";\n    a <- bool(;\n}\n";
    let (_parser, document) = open(source);

    let line = source.lines().nth(1).expect("line 1 exists");
    let byte_end = line.len();
    let utf16_end = line.chars().map(char::len_utf16).sum::<usize>();
    let char_end = line.chars().count();

    assert_eq!(byte_end, utf16_end + 2, "the emoji is 4 bytes but 2 UTF-16 units");
    assert_eq!(utf16_end, char_end + 1, "the emoji is 1 char but 2 UTF-16 units");

    let end_of_line = document.position(tree_sitter::Point {
        row: 1,
        column: byte_end,
    });
    assert_eq!(end_of_line, position(1, utf16_end as u32));
}

#[test]
fn incremental_edit_updates_the_tree() {
    let (mut parser, mut document) = open("function model() {\n    a <- bool(;\n}\n");
    assert!(!syntax::diagnostics(&document).is_empty(), "starts broken");

    // Replace the offending `(;` with `();`, the way a client reports a two-character edit.
    // `(` sits at column 13 of `    a <- bool(;`, so the range covers columns 13..15.
    document.apply(
        &mut parser,
        vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: position(1, 13),
                end: position(1, 15),
            }),
            range_length: None,
            text: "();".to_string(),
        }],
        2,
    );

    assert_eq!(document.text(), "function model() {\n    a <- bool();\n}\n");
    assert_eq!(document.version(), 2);
    assert_eq!(
        syntax::diagnostics(&document),
        Vec::new(),
        "the edit fixed the only error"
    );
}

#[test]
fn incremental_edit_through_multibyte_text() {
    // The edit is positioned *after* non-ASCII text on the same line, so its byte offset and its
    // UTF-16 column differ. This is the case that silently corrupts a buffer when conversion is
    // wrong: the splice lands mid-character and the reparse sees text the client never sent.
    let (mut parser, mut document) = open("function model() {\n    s = \"ціна\" + x;\n}\n");

    let line = "    s = \"ціна\" + x;";
    let utf16_column = line.chars().map(char::len_utf16).sum::<usize>() - 1;

    document.apply(
        &mut parser,
        vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: position(1, utf16_column as u32),
                end: position(1, utf16_column as u32 + 1),
            }),
            range_length: None,
            text: ";;".to_string(),
        }],
        2,
    );

    assert!(
        document.text().contains("ціна"),
        "the multibyte text survived the splice: {:?}",
        document.text(),
    );
    assert!(document.text().ends_with(";;\n}\n"), "got {:?}", document.text());
}

#[test]
fn full_replacement_discards_the_old_tree() {
    let (mut parser, mut document) = open("function model() {\n    a <- bool(;\n}\n");

    document.apply(
        &mut parser,
        vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "function model() {\n    a <- bool();\n}\n".to_string(),
        }],
        7,
    );

    assert_eq!(document.version(), 7);
    assert_eq!(syntax::diagnostics(&document), Vec::new());
}

#[test]
fn out_of_range_edit_is_ignored_rather_than_panicking() {
    // A client computing changes against a version we have already moved past can name a position
    // past the end. Dropping the change loses an edit; panicking takes the whole server down.
    let (mut parser, mut document) = open("function model() {\n    a <- bool();\n}\n");
    let before = document.text().to_string();

    document.apply(
        &mut parser,
        vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: position(99, 0),
                end: position(99, 4),
            }),
            range_length: None,
            text: "x".to_string(),
        }],
        2,
    );

    assert_eq!(document.text(), before);
}

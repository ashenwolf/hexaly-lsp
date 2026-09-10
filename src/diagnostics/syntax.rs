//! Syntax diagnostics from the parse tree.
//!
//! This is the layer that carries the interactive experience. Unlike the Hexaly compiler \u2014 which
//! reports one error per parse, with a line and no column \u2014 tree-sitter recovers and keeps going,
//! so a file with four mistakes shows four squiggles, each with a real range.
//!
//! The two layers publish under separate diagnostic sources so they can be cleared independently.
//! Sharing a source would let a stale compiler diagnostic outlive the edit that fixed it.

use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity};

use crate::document::Document;

pub const SOURCE: &str = "hexaly-syntax";

/// Every `ERROR` and `MISSING` node in the tree, as diagnostics.
///
/// tree-sitter distinguishes the two and the distinction is worth surfacing: `MISSING` means the
/// parser knows exactly which token should be there, which is a more useful message than "syntax
/// error". `MISSING` nodes are also zero-width, so their range is widened to the following
/// character \u2014 an empty range renders as an invisible squiggle in most clients.
pub fn diagnostics(document: &Document) -> Vec<Diagnostic> {
    let tree = document.tree();

    // `has_error` is a cheap flag on the root; walking the whole tree on every keystroke of a
    // clean file would be wasted work.
    if !tree.root_node().has_error() {
        return Vec::new();
    }

    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    let mut diagnostics = Vec::new();

    while let Some(node) = stack.pop() {
        if node.is_missing() {
            diagnostics.push(Diagnostic {
                range: document.range(node),
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some(SOURCE.to_string()),
                message: format!("missing {}", node.kind()),
                ..Diagnostic::default()
            });
            continue;
        }

        if node.is_error() {
            diagnostics.push(Diagnostic {
                range: document.range(node),
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some(SOURCE.to_string()),
                message: "syntax error".to_string(),
                ..Diagnostic::default()
            });
            // An ERROR node's children are the tokens the parser could not place. They are not
            // themselves mistakes, and reporting them would bury the one real error.
            continue;
        }

        // Only descend where an error actually lives. `has_error` is true for every ancestor of a
        // problem, so this prunes whole clean subtrees instead of visiting every node.
        stack.extend(node.children(&mut cursor).filter(|child| child.has_error()));
    }

    // The walk is depth-first with a stack, so results come out in an order that reflects the
    // traversal rather than the file. Clients group by position, and so should we.
    diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start.line, diagnostic.range.start.character));
    diagnostics
}

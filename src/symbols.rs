//! Symbols declared in the document itself.
//!
//! Completion that offers only the standard library is the less useful half: what a modeller reaches
//! for most is the decision variable they declared four lines up. These come from the tree, so they
//! are available per keystroke and need no Hexaly installation.
//!
//! Scoping is deliberately flat \u2014 every declaration in the file, regardless of nesting. Real scope
//! analysis is worth doing when it buys something (go-to-definition, rename), but for completion an
//! over-broad candidate list costs a spurious suggestion while a too-narrow one hides the name the
//! user wants.

use crate::document::Document;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    pub name: String,
    pub kind: LocalKind,
    /// The declaration as written, for the completion item's detail line.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    Function,
    Class,
    /// A model decision or intermediate expression: `x[i in 0...n] <- bool()`.
    Decision,
    Variable,
    Module,
}

/// Declarations in `document`, in source order and deduplicated by name.
///
/// The node kinds come from the grammar: `function_declaration`, `class_declaration`,
/// `indexed_declaration` (a declaration over an index space), `local_declaration` and
/// `use_statement`.
pub fn locals(document: &Document) -> Vec<Local> {
    let tree = document.tree();
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    let mut found: Vec<Local> = Vec::new();

    while let Some(node) = stack.pop() {
        if let Some(local) = declaration(document, node) {
            // First declaration wins: a name redeclared later is the same name, and Hexaly rejects
            // genuine duplicates anyway.
            if !found.iter().any(|existing| existing.name == local.name) {
                found.push(local);
            }
        }

        stack.extend(node.children(&mut cursor));
    }

    // The walk is a stack, so order reflects traversal rather than the file.
    found.sort_by_key(|local| local.name.clone());
    found
}

fn declaration(document: &Document, node: tree_sitter::Node<'_>) -> Option<Local> {
    let kind = match node.kind() {
        "function_declaration" => LocalKind::Function,
        "class_declaration" => LocalKind::Class,
        "indexed_declaration" => LocalKind::Decision,
        "declarator" => LocalKind::Variable,
        "use_statement" => return module(document, node),
        _ => return None,
    };

    let name = node.child_by_field_name("name")?;
    let text = document.node_text(name)?;

    Some(Local {
        name: text.to_string(),
        kind,
        detail: summarise(document, node),
    })
}

/// A `use` statement contributes the name a qualified call would start with: the alias when there is
/// one, otherwise the last segment of the module path (`use utils.fn;` is reached as `fn`).
fn module(document: &Document, node: tree_sitter::Node<'_>) -> Option<Local> {
    let name = match node.child_by_field_name("alias") {
        Some(alias) => document.node_text(alias)?.to_string(),
        None => {
            let path = node.child_by_field_name("module")?;
            document.node_text(path)?.rsplit('.').next()?.to_string()
        }
    };

    Some(Local {
        name,
        kind: LocalKind::Module,
        detail: summarise(document, node),
    })
}

/// The declaration's first line, trimmed. Enough to tell `x <- bool()` from `x <- float(0, 1)`
/// without pulling a whole function body into a completion list.
fn summarise(document: &Document, node: tree_sitter::Node<'_>) -> String {
    document
        .node_text(node)
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('{')
        .trim()
        .to_string()
}

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
    /// For a `use` statement, the dotted module path it names — which is what resolves to a file.
    /// The alias is in `name`, so `use utils.functionalUtils as fn` gives `fn` and
    /// `utils.functionalUtils` respectively, and both are needed: one to recognise `fn.` and the
    /// other to find the file.
    pub module_path: Option<String>,
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

/// Declarations at the top level of the file only.
///
/// This is the module's public surface: what an importer can reach as `alias.name`. A variable
/// declared inside a function body is not reachable that way, so including it would offer a
/// candidate that cannot compile.
///
/// Distinct from `locals`, which walks the whole tree on purpose — inside the file being edited, a
/// name declared four lines up in the same function is exactly what completion should offer.
pub fn top_level(document: &Document) -> Vec<Local> {
    let tree = document.tree();
    let mut cursor = tree.walk();
    let mut inner = tree.walk();

    let mut found: Vec<Local> = tree
        .root_node()
        .children(&mut cursor)
        .flat_map(|node| {
            // A top-level `local a = 1, b = 2;` wraps its declarators in a `local_declaration`, so
            // that one node is descended into. Nothing else is: a `block` or a function body would
            // reintroduce exactly the names this function exists to exclude.
            let children: Vec<tree_sitter::Node> = if node.kind() == "local_declaration" {
                node.children(&mut inner).collect()
            } else {
                vec![node]
            };

            children
                .into_iter()
                .filter_map(|child| declaration(document, child))
                .collect::<Vec<_>>()
        })
        .collect();

    found.sort_by_key(|local| local.name.clone());
    found.dedup_by(|left, right| left.name == right.name);
    found
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
        module_path: None,
    })
}

/// A `use` statement contributes the name a qualified call would start with: the alias when there is
/// one, otherwise the last segment of the module path (`use utils.fn;` is reached as `fn`).
fn module(document: &Document, node: tree_sitter::Node<'_>) -> Option<Local> {
    let path = node.child_by_field_name("module")?;
    let module_path = document.node_text(path)?.to_string();

    let name = match node.child_by_field_name("alias") {
        Some(alias) => document.node_text(alias)?.to_string(),
        None => module_path.rsplit('.').next()?.to_string(),
    };

    Some(Local {
        name,
        kind: LocalKind::Module,
        detail: summarise(document, node),
        module_path: Some(module_path),
    })
}

/// Members of a class declared in this document, for completion after `ClassName.`.
///
/// Locally declared classes are containers just as much as the library's are: the production model
/// this was built against reaches `SpecialLocations.PICK_MANUAL` far more often than any stdlib
/// class. Fields, methods and the constructor all come from the grammar's `class_body`.
pub fn class_members(document: &Document, class_name: &str) -> Vec<Local> {
    let tree = document.tree();
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];

    while let Some(node) = stack.pop() {
        if node.kind() == "class_declaration" {
            let matches = node
                .child_by_field_name("name")
                .and_then(|name| document.node_text(name))
                .is_some_and(|name| name == class_name);

            if matches {
                return node
                    .child_by_field_name("body")
                    .map(|body| members(document, body))
                    .unwrap_or_default();
            }
        }

        stack.extend(node.children(&mut cursor));
    }

    Vec::new()
}

fn members(document: &Document, body: tree_sitter::Node<'_>) -> Vec<Local> {
    let mut cursor = body.walk();

    body.children(&mut cursor)
        .filter_map(|member| {
            let kind = match member.kind() {
                "field_declaration" => LocalKind::Variable,
                "method_declaration" => LocalKind::Function,
                _ => return None,
            };

            let name = member.child_by_field_name("name")?;

            Some(Local {
                name: document.node_text(name)?.to_string(),
                kind,
                detail: summarise(document, member),
                module_path: None,
            })
        })
        .collect()
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

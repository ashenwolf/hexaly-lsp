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

use tower_lsp_server::ls_types::Range;

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
    /// A whole module, bound by `use path as alias;`.
    Module,
    /// A single member imported from a module, bound by `use Name from path;`. Distinct from
    /// `Module` because it resolves one level deeper: to a declaration inside that file, not to the
    /// file's own surface.
    Import,
}

/// A declaration and what it contains, for an outline.
///
/// Nesting is why this exists rather than reusing `top_level`: a class with eight fields renders as
/// eight siblings in a flat list, and the outline of a Hexaly model is mostly "which function, and
/// what does it decide" — in the production model this targets, every decision lives inside a
/// function, so a flat list of decisions has no structure to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outline {
    pub name: String,
    pub kind: LocalKind,
    pub detail: String,
    /// The whole declaration, for selecting it.
    pub range: Range,
    /// Just the name, for placing the cursor.
    pub selection: Range,
    pub children: Vec<Outline>,
}

/// The document's structure as a tree.
///
/// Classes contain their members; functions contain the decisions and objectives declared inside
/// them. Statements are not recursed into beyond that: a decision inside a nested `for` inside a
/// function is still shown under the function, because the intervening block is not something a
/// reader navigates to.
pub fn outline(document: &Document) -> Vec<Outline> {
    let tree = document.tree();
    let mut cursor = tree.walk();

    tree.root_node()
        .children(&mut cursor)
        .flat_map(|node| outline_node(document, node))
        .collect()
}

fn outline_node(document: &Document, node: tree_sitter::Node<'_>) -> Vec<Outline> {
    // A top-level `local a = 1, b = 2;` wraps its declarators, so that one node is descended into.
    if node.kind() == "local_declaration" {
        let mut cursor = node.walk();
        return node
            .children(&mut cursor)
            .flat_map(|child| outline_node(document, child))
            .collect();
    }

    let Some(local) = declarations(document, node).into_iter().next() else {
        return Vec::new();
    };

    // `use` statements are bindings, not structure: an outline listing them buries the declarations
    // a reader opened it to find.
    if matches!(local.kind, LocalKind::Module | LocalKind::Import) {
        return Vec::new();
    }

    let children = match node.kind() {
        "class_declaration" => node
            .child_by_field_name("body")
            .map(|body| contained(document, body))
            .unwrap_or_default(),
        "function_declaration" => node
            .child_by_field_name("body")
            .map(|body| contained(document, body))
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let selection = node
        .child_by_field_name("name")
        .map(|name| document.range(name))
        .unwrap_or_else(|| document.range(node));

    vec![Outline {
        name: local.name,
        kind: local.kind,
        detail: local.detail,
        range: document.range(node),
        selection,
        children,
    }]
}

/// Declarations inside a class body or a function body, one level down.
///
/// `objective_statement` is included even though it has no name, because `minimize` and `maximize`
/// are the point of a model and a reader looks for them: they are labelled by their direction.
fn contained(document: &Document, body: tree_sitter::Node<'_>) -> Vec<Outline> {
    let mut cursor = body.walk();
    let mut stack: Vec<tree_sitter::Node> = body.children(&mut cursor).collect();
    let mut found = Vec::new();

    while let Some(node) = stack.pop() {
        if node.kind() == "objective_statement" {
            if let Some(direction) = node.child_by_field_name("direction") {
                found.push(Outline {
                    name: document.node_text(direction).unwrap_or("objective").to_string(),
                    kind: LocalKind::Decision,
                    detail: summarise(document, node),
                    range: document.range(node),
                    selection: document.range(direction),
                    children: Vec::new(),
                });
            }
            continue;
        }

        // Only decisions and members are worth listing; a plain local inside a function body is
        // noise in an outline, however useful it is in completion.
        let interesting = matches!(
            node.kind(),
            "indexed_declaration" | "field_declaration" | "method_declaration" | "constructor_declaration"
        );

        if interesting {
            found.extend(outline_member(document, node));
            continue;
        }

        // Descend through control flow: a decision inside a `for` is still the function's structure.
        if matches!(
            node.kind(),
            "for_statement" | "if_statement" | "while_statement" | "block"
        ) {
            let mut inner = node.walk();
            stack.extend(node.children(&mut inner));
        }
    }

    found.sort_by_key(|item| (item.range.start.line, item.range.start.character));
    found
}

fn outline_member(document: &Document, node: tree_sitter::Node<'_>) -> Option<Outline> {
    let kind = match node.kind() {
        "indexed_declaration" => LocalKind::Decision,
        "field_declaration" => LocalKind::Variable,
        "method_declaration" => LocalKind::Function,
        "constructor_declaration" => LocalKind::Function,
        _ => return None,
    };

    // A constructor has no name field; it is named for what it is.
    let name_node = node.child_by_field_name("name");
    let name = match name_node {
        Some(name) => document.node_text(name)?.to_string(),
        None if node.kind() == "constructor_declaration" => "constructor".to_string(),
        None => return None,
    };

    Some(Outline {
        name,
        kind,
        detail: summarise(document, node),
        range: document.range(node),
        selection: name_node
            .map(|name| document.range(name))
            .unwrap_or_else(|| document.range(node)),
        children: Vec::new(),
    })
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
                .flat_map(|child| declarations(document, child))
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
        for local in declarations(document, node) {
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

/// The declarations a node introduces. A `use` statement can introduce several — `use A, B from m;`
/// is legal — so this returns a list rather than an option.
fn declarations(document: &Document, node: tree_sitter::Node<'_>) -> Vec<Local> {
    let kind = match node.kind() {
        "function_declaration" => LocalKind::Function,
        "class_declaration" => LocalKind::Class,
        "indexed_declaration" => LocalKind::Decision,
        "declarator" => LocalKind::Variable,
        "use_statement" => return module(document, node).unwrap_or_default(),
        _ => return Vec::new(),
    };

    node.child_by_field_name("name")
        .and_then(|name| document.node_text(name))
        .map(|text| {
            vec![Local {
                name: text.to_string(),
                kind,
                detail: summarise(document, node),
                module_path: None,
            }]
        })
        .unwrap_or_default()
}

/// A `use` statement contributes the names a qualified reference can start with.
///
/// Two distinct forms, and they mean different things:
///
/// - `use utils.functionalUtils as fn;` binds the whole module, so `fn.` reaches the module's
///   declarations. The alias (or the last path segment) is the name.
/// - `use SpecialLocations from inputs.specialLocations;` binds *one member* of that module, so
///   `SpecialLocations.` reaches the members of that class, one level deeper.
///
/// The second form yields an `Import` rather than a `Module` so the resolver can tell them apart:
/// treating it as a module would list the file's declarations, which for a class import means
/// offering the class itself as its own member.
fn module(document: &Document, node: tree_sitter::Node<'_>) -> Option<Vec<Local>> {
    let path = node.child_by_field_name("module")?;
    let module_path = document.node_text(path)?.to_string();
    let detail = summarise(document, node);

    let mut cursor = node.walk();
    let specifiers: Vec<tree_sitter::Node> = node
        .children(&mut cursor)
        .filter(|child| child.kind() == "import_specifier")
        .collect();

    if !specifiers.is_empty() {
        return Some(
            specifiers
                .into_iter()
                .filter_map(|specifier| {
                    // `use X as Y from m;` is legal, and the alias is what appears in code.
                    let name = specifier
                        .child_by_field_name("alias")
                        .or_else(|| specifier.child_by_field_name("name"))?;

                    Some(Local {
                        name: document.node_text(name)?.to_string(),
                        kind: LocalKind::Import,
                        detail: detail.clone(),
                        module_path: Some(module_path.clone()),
                    })
                })
                .collect(),
        );
    }

    let name = match node.child_by_field_name("alias") {
        Some(alias) => document.node_text(alias)?.to_string(),
        None => module_path.rsplit('.').next()?.to_string(),
    };

    Some(vec![Local {
        name,
        kind: LocalKind::Module,
        detail,
        module_path: Some(module_path),
    }])
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

/// The declaration's first line, trimmed to its signature.
///
/// A single-line function carries its whole body on that line, and a signature-help popup showing
/// `function f(a) { return a; }` buries the parameter list it exists to show. Everything from the
/// opening brace is dropped, which for a declaration is always the body.
fn summarise(document: &Document, node: tree_sitter::Node<'_>) -> String {
    let first = document
        .node_text(node)
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .trim();

    // Split on the brace rather than trimming it, so a one-line body goes too. A declaration cannot
    // contain a brace before its body, so this cannot cut a signature short.
    first
        .split_once('{')
        .map_or(first, |(signature, _)| signature)
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_string()
}

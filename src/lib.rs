//! Language server for Hexaly Modeler models (`.hxm`).
//!
//! Phase 1 serves syntax diagnostics from the tree-sitter grammar alone, so it is useful on a
//! machine with no Hexaly installation and no licence. The compiler-backed layer is additive.
//!
//! Exposed as a library as well as a binary so the coordinate conversions and diagnostic mapping
//! can be tested directly, rather than only through a live stdio session.

pub mod diagnostics;
pub mod document;
pub mod server;

/// The grammar, ready to parse. Every parser in the process is built here so a version mismatch
/// between the grammar and the tree-sitter runtime surfaces once, at startup.
pub fn parser() -> tree_sitter::Parser {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_hexaly::LANGUAGE.into())
        .expect("the bundled grammar is built against this tree-sitter version");
    parser
}

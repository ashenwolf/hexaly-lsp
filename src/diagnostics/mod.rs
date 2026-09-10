//! Diagnostics sources.
//!
//! Two layers, deliberately siblings rather than one falling back to the other. tree-sitter
//! recovers and reports every syntax error at once with real character ranges; the Hexaly frontend
//! reports one error per run with a line and no column, but knows things the grammar cannot
//! (duplicate declarations, whether a `use` resolves). Neither subsumes the other.
//!
//! They publish under separate `source` values so each can be retracted on its own. Sharing one
//! would let a compiler diagnostic outlive the edit that fixed it, since the two layers run at
//! different times — syntax per keystroke, the compiler on save.

pub mod hexaly;
pub mod syntax;

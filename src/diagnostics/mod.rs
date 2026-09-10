//! Diagnostics sources.
//!
//! Phase 1 ships the syntax layer only. The Hexaly compiler layer joins it here, deliberately as
//! a sibling rather than a fallback: it is authoritative for things the grammar cannot know
//! (duplicate declarations, whether a `use` resolves) but reports one line-granular error per
//! parse, so neither layer subsumes the other.

pub mod syntax;

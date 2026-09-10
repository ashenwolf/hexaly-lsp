//! The Hexaly standard library, as documented by an installation.
//!
//! Extracted offline by `tools/scrape-stdlib.py` into a JSON artifact that is committed and
//! compiled in. Two reasons it is not scraped at runtime: the documentation belongs to an
//! installation the user may not have, and it is Sphinx output whose structure can change between
//! releases. Doing it offline means a documentation change breaks the scraper, visibly, rather than
//! the language server.
//!
//! The artifact is version-stamped so a mismatch between it and a user's Hexaly is at least
//! reportable, rather than silently offering symbols that do not exist.

use std::sync::LazyLock;

use serde::Deserialize;

/// Hexaly 14.0. Adding a version means running the scraper against that installation and choosing
/// between artifacts here; the shape is stable enough that they can coexist.
const ARTIFACT: &str = include_str!("hexaly-14.json");

#[derive(Debug, Deserialize)]
pub struct Library {
    pub version: String,
    pub symbols: Vec<Symbol>,
}

/// What kind of thing a symbol is, in Hexaly's own vocabulary. Mapped onto LSP's
/// `CompletionItemKind` at the point of use, so this stays a record of what the documentation says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Function,
    Method,
    Variable,
    Constant,
    Field,
    Class,
    Operator,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Symbol {
    pub name: String,
    /// The module or class holding this symbol (`io`, `HxExpression`), empty for a global.
    pub container: String,
    pub kind: Kind,
    /// Every documented overload, rendered as Hexaly writes them. Variadics keep their brackets
    /// (`println([arg0[, arg1[, ...]]])`) because that is how the documentation expresses "and so
    /// on", and rewriting it would claim an arity the function does not have.
    pub signatures: Vec<String>,
    pub parameters: Vec<Parameter>,
    pub returns: String,
    pub documentation: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub r#type: String,
    pub doc: String,
}

impl Symbol {
    pub fn is_global(&self) -> bool {
        self.container.is_empty()
    }

    /// Markdown for a hover: the signatures as code, then the prose, then the parameters and return
    /// type as a list. Assembled rather than stored so the artifact holds facts, not presentation.
    pub fn hover(&self) -> String {
        let mut markdown = format!("```hexaly\n{}\n```", self.signatures.join("\n"));

        if !self.documentation.is_empty() {
            markdown.push_str("\n\n");
            markdown.push_str(&self.documentation);
        }

        if !self.parameters.is_empty() {
            markdown.push('\n');
            for parameter in &self.parameters {
                let annotation = if parameter.r#type.is_empty() {
                    String::new()
                } else {
                    format!(" *{}*", parameter.r#type)
                };
                markdown.push_str(&format!(
                    "\n- `{}`{} \u{2014} {}",
                    parameter.name, annotation, parameter.doc
                ));
            }
        }

        if !self.returns.is_empty() {
            markdown.push_str(&format!("\n\nReturns *{}*", self.returns));
        }

        markdown
    }

    /// The one-line detail shown beside a completion item: the first signature, plus the return
    /// type when there is one.
    pub fn detail(&self) -> String {
        let signature = self.signatures.first().cloned().unwrap_or_else(|| self.name.clone());

        if self.returns.is_empty() {
            signature
        } else {
            format!("{} \u{2192} {}", signature, self.returns)
        }
    }
}

/// Parsed once. The artifact is compiled in, so a failure here is a build-time mistake that would
/// fail every test rather than something a user can trigger.
pub static LIBRARY: LazyLock<Library> =
    LazyLock::new(|| serde_json::from_str(ARTIFACT).expect("the bundled stdlib artifact is valid JSON"));

/// Globals: everything callable or referenceable without a qualifier. Offered when the cursor is
/// not after a dot.
pub fn globals() -> impl Iterator<Item = &'static Symbol> {
    LIBRARY.symbols.iter().filter(|symbol| symbol.is_global())
}

/// Members of `container`, for completion after a dot. Matched case-sensitively because Hexaly
/// identifiers are.
pub fn members(container: &str) -> impl Iterator<Item = &'static Symbol> + '_ {
    LIBRARY
        .symbols
        .iter()
        .filter(move |symbol| symbol.container == container)
}

/// Every symbol with this name, in any container. A bare name is ambiguous \u2014 `close` exists on
/// several classes \u2014 so hover has to consider all of them.
pub fn named(name: &str) -> impl Iterator<Item = &'static Symbol> + '_ {
    LIBRARY.symbols.iter().filter(move |symbol| symbol.name == name)
}

/// The names that can appear before a dot: modules and classes the library documents.
pub fn containers() -> impl Iterator<Item = &'static str> {
    LIBRARY
        .symbols
        .iter()
        .map(|symbol| symbol.container.as_str())
        .filter(|container| !container.is_empty())
}

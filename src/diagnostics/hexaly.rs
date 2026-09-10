//! Compiler-backed diagnostics: what the grammar cannot know.
//!
//! The Hexaly frontend is authoritative for duplicate declarations and module resolution, and
//! blind to everything inside a function body. It also reports exactly one error per run, with a
//! line and no column. So this layer is a complement to the syntax layer, never a replacement, and
//! publishes under its own source so the two can be retracted independently.
//!
//! # The load-only invocation
//!
//! The CLI has no parse-only flag; its usage is `hexaly file.hxm [globals]`, and on a licensed
//! machine that runs the model. Instead we write a wrapper beside the target and run that:
//!
//! ```text
//! use <module>;
//! function main() {}
//! ```
//!
//! `use` binds the target's declarations - the parse and declaration-binding work we want - and
//! then our own empty `main` runs and exits. Three properties were measured rather than assumed:
//!
//! - The empty `main` is load-bearing. Without it Hexaly falls through to solving and demands a
//!   licence.
//! - The target's own `model()` body never executes: a `model()` that writes a file leaves nothing
//!   behind, while our `main` writing one does. `use` binds without invoking.
//! - Exit status is 1 for a rejected module and 0 for a clean one, so failure need not be inferred
//!   from the shape of the output.
//!
//! The wrapper must be a real file in the target's own directory. Hexaly resolves `use` against
//! the directory of the script it was given, so a wrapper in a temp directory cannot see the
//! target, and `/dev/stdin` or a process substitution has no directory at all - both fall through
//! to solving. That is the reason for a file on disk rather than a pipe.

use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const SOURCE: &str = "hexaly";

/// The wrapper's own name is free - it is an entry point, not a module - so it is dot-prefixed to
/// stay out of file listings, and specific enough not to collide with anything a user would write.
const WRAPPER: &str = ".hexaly-lsp-probe.hxm";

/// One compiler complaint, already resolved to the file it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerError {
    /// Zero-based, converted from Hexaly's one-based line for direct use as an LSP line.
    pub line: u32,
    pub message: String,
    /// The file the innermost frame named, as Hexaly spelled it.
    pub file: String,
}

/// Runs the load-only invocation against `target` and returns the single error, if any.
///
/// `None` covers both "the module is fine" and "we could not ask" - a missing binary, an
/// unwritable directory. Neither is worth surfacing as a diagnostic on the user's code.
pub async fn check(hexaly: &Path, target: &Path) -> Option<CompilerError> {
    let directory = target.parent()?;
    let module = module_name(target)?;

    let wrapper = directory.join(WRAPPER);
    let source = format!("use {module};\nfunction main() {{}}\n");
    tokio::fs::write(&wrapper, source).await.ok()?;

    let output = tokio::process::Command::new(hexaly)
        .arg(WRAPPER)
        // Hexaly resolves modules against the script's directory, and the wrapper is in the
        // target's directory, so this is also where the target's own `use` statements resolve.
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    // Best-effort: leaving a stale wrapper behind is untidy but harmless, and a failure to remove
    // it must not suppress a diagnostic we already have.
    let _ = tokio::fs::remove_file(&wrapper).await;

    let output = output.ok()?;
    if output.status.success() {
        return None;
    }

    // Hexaly writes its banner and errors to stdout; stderr is included because which stream
    // carries a given failure is not documented.
    let combined = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);

    parse_error(&combined, &wrapper_name_for(target))
}

/// A module name is the filename without extension, and must be a Hexaly identifier: a letter or
/// underscore followed by alphanumerics or underscores.
///
/// So `my-model.hxm` and `my.model.hxm` cannot be loaded as modules at all - Hexaly rejects the
/// `use` statement itself. Returning `None` skips this layer for such files rather than reporting
/// the wrapper's own syntax error as if it were the user's.
fn module_name(target: &Path) -> Option<String> {
    let stem = target.file_stem()?.to_str()?;

    let mut characters = stem.chars();
    let first = characters.next()?;
    let valid = (first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_');

    valid.then(|| stem.to_string())
}

fn wrapper_name_for(target: &Path) -> String {
    target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Extracts the innermost frame of a Hexaly error.
///
/// Errors nest once per `use` hop, arbitrarily deep:
///
/// ```text
/// Error: At line 1, file '_p.hxm': At line 1, file './/mid.hxm': At line 1, file './/dep.hxm': Cannot load module 'x': ...
/// ```
///
/// The outer frames are the wrapper and any intermediate modules; only the last names the real
/// location. Paths in inner frames carry a `.//` prefix from the lookup path, which is stripped.
fn parse_error(output: &str, target_name: &str) -> Option<CompilerError> {
    let line = output.lines().find(|line| line.contains("At line "))?;

    // Walk every frame, keeping the last: that is the innermost, and its trailing text is the
    // message. A `rfind` would be shorter but could not also recover the file name.
    let mut rest = line;
    let mut innermost = None;

    while let Some(start) = rest.find("At line ") {
        let after = &rest[start + "At line ".len()..];
        let (number, after) = after.split_once(',')?;
        let after = after.trim_start().strip_prefix("file '")?;
        let (file, after) = after.split_once("':")?;

        innermost = Some((number.trim().parse::<u32>().ok()?, normalise(file), after.trim()));
        rest = after;
    }

    let (line_number, file, message) = innermost?;

    // A frame naming a file other than the one under diagnosis is a genuine error in a *dependency*
    // - reporting it on this file's line 3 would point at unrelated code. It is dropped rather than
    // misplaced; surfacing it properly means a diagnostic on the `use` statement instead.
    if !file.ends_with(target_name) {
        return None;
    }

    Some(CompilerError {
        // Hexaly counts from 1, LSP from 0. `saturating_sub` because a malformed 0 would otherwise
        // wrap to the last line of the file.
        line: line_number.saturating_sub(1),
        message: message.to_string(),
        file,
    })
}

fn normalise(file: &str) -> String {
    file.trim_start_matches("./").trim_start_matches('/').to_string()
}

/// Where the diagnostic goes when the innermost frame names a dependency rather than this file:
/// the `use` statement that pulled it in. Resolving it to a node is the caller's job, since only
/// it has the tree.
pub fn failed_module(message: &str) -> Option<&str> {
    message
        .strip_prefix("Cannot load module '")?
        .split_once('\'')
        .map(|(module, _)| module)
}

/// The wrapper path this layer would use for `target`, exposed so a caller can exclude it from
/// file watching and so tests can assert it is cleaned up.
pub fn wrapper_path(target: &Path) -> Option<PathBuf> {
    Some(target.parent()?.join(WRAPPER))
}

/// Whether this layer can diagnose `target` at all, which turns on the filename being usable as a
/// module name. Exposed so a caller can tell "nothing to report" apart from "cannot be asked".
pub fn is_diagnosable(target: &Path) -> bool {
    module_name(target).is_some()
}

//! The formatter, against the 66 models Hexaly ships.
//!
//! These are the reference for what formatted Hexaly looks like, and the only corpus large enough to
//! find the cases hand-written fixtures miss. Two properties are asserted for every file, and they
//! are the ones that make a formatter safe to bind to save:
//!
//! - token-equivalence: formatting moved whitespace and nothing else
//! - idempotence: formatting formatted output changes nothing further
//!
//! A third is checked and reported rather than asserted: how many of these already-formatted files
//! come back byte-identical. That number is the honest measure of whether the rules match Hexaly's
//! own style, and a regression in it is visible without being fatal.

use std::path::{Path, PathBuf};

use hexaly_lsp::{format, token};

/// Hexaly's bundled examples, if this machine has an installation.
///
/// Located the same way `tests/examples.rs` does, rather than through `discovery`: that module finds
/// the *interpreter* for running the compiler, which can be on `$PATH` with no examples beside it.
fn corpus() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::fs::read_dir("/opt")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("hexaly_"))
        })
        .map(|path| path.join("examples"))
        .filter(|path| path.is_dir())
        .collect();

    roots.sort();

    let Some(examples) = roots.pop() else {
        return Vec::new();
    };

    let mut models = Vec::new();
    collect(&examples, &mut models);
    models.sort();
    models
}

fn collect(directory: &Path, models: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, models);
        } else if path.extension().is_some_and(|extension| extension == "hxm") {
            models.push(path);
        }
    }
}

#[test]
fn formatting_never_changes_the_tokens() {
    let models = corpus();
    if models.is_empty() {
        eprintln!("no Hexaly installation; skipping the corpus formatting test");
        return;
    }

    let mut changed = Vec::new();

    for path in &models {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };

        // `format` returns None when the file is already formatted, which is itself the signal being
        // measured below.
        let Some(formatted) = format::format(&source) else {
            continue;
        };

        assert!(
            token::equivalent(&source, &formatted),
            "formatting {} changed its tokens",
            path.display()
        );

        // Idempotence: the second pass must find nothing to do. A formatter that oscillates makes
        // every save a diff.
        assert!(
            format::format(&formatted).is_none(),
            "formatting {} is not idempotent",
            path.display()
        );

        changed.push(path);
    }

    // Reported, not asserted. Hexaly's own models are the style reference, so the fewer this
    // rewrites the better the rules match - but a handful differ over things the corpus is genuinely
    // inconsistent about, and failing on that would be asserting my taste over theirs.
    eprintln!(
        "corpus: {} of {} models already formatted; {} rewritten",
        models.len() - changed.len(),
        models.len(),
        changed.len()
    );

    for path in changed.iter().take(10) {
        eprintln!("  rewritten: {}", path.display());

        // The first differing line, because "this file changed" is not actionable and the diff is
        // how a rule is judged against Hexaly's own style.
        if let Ok(source) = std::fs::read_to_string(path)
            && let Some(formatted) = format::format(&source)
            && let Some((before, after)) = source
                .lines()
                .zip(formatted.lines())
                .find(|(before, after)| before != after)
        {
            eprintln!("    -{before:?}\n    +{after:?}");
        }
    }
}

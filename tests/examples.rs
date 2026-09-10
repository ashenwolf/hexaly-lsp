//! Runs the compiler layer over every model shipped with a local Hexaly install.
//!
//! The unit tests prove the invocation works on cases we constructed; this proves it stays quiet on
//! 66 models we did not write. A false positive here would be worse than no diagnostics at all,
//! since a wrong squiggle on correct code costs more trust than a missing one.
//!
//! Self-skips without an install, so CI is unaffected.

use std::path::PathBuf;

use hexaly_lsp::diagnostics::hexaly;
use hexaly_lsp::discovery;

fn examples_root() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir("/opt")
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

    candidates.sort();
    candidates.pop()
}

#[tokio::test]
async fn shipped_examples_produce_no_false_positives() {
    let (Some(binary), Some(root)) = (discovery::locate(None).path().map(PathBuf::from), examples_root()) else {
        eprintln!("no Hexaly install; skipping");
        return;
    };

    // Each example lives in its own directory alongside its data files, and is diagnosed in place:
    // copying elsewhere would break the `use` resolution this layer depends on.
    let models: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("examples directory is readable")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .flat_map(|directory| {
            std::fs::read_dir(&directory)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "hxm"))
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(!models.is_empty(), "no models under {}", root.display());

    let mut complaints = Vec::new();
    let mut checked = 0;

    for model in &models {
        // A file whose name is not a Hexaly identifier cannot be loaded as a module at all, so this
        // layer declines it rather than reporting the wrapper's own syntax error. Counted so the
        // assertion below cannot be satisfied by silently checking nothing.
        if !hexaly::is_diagnosable(model) {
            continue;
        }

        match hexaly::check(&binary, model).await {
            Some(error) => complaints.push(format!(
                "{}:{}: {}",
                model.strip_prefix(&root).unwrap_or(model).display(),
                error.line + 1,
                error.message,
            )),
            None => checked += 1,
        }
    }

    assert!(
        complaints.is_empty(),
        "{} of {} shipped models were reported broken:\n{}",
        complaints.len(),
        models.len(),
        complaints.join("\n"),
    );

    // The examples include `clustered-vehicle-routing.hxm`, whose hyphens make it unusable as a
    // module name, so a small shortfall is expected. A large one means the checks are not running.
    assert!(
        checked >= models.len() - 2,
        "only {checked} of {} models were actually checked",
        models.len(),
    );
}

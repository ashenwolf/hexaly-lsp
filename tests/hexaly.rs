//! Tests for the compiler-backed layer and Hexaly discovery.
//!
//! The error-parsing tests use output captured verbatim from Hexaly 14.0 rather than output this
//! server generates, so they pin the real format. The end-to-end tests self-skip when no Hexaly is
//! installed, which is the normal state on CI.

use std::path::{Path, PathBuf};

use hexaly_lsp::diagnostics::hexaly;
use hexaly_lsp::discovery::{self, Discovery};

/// The newest local install, or `None` to skip. Mirrors the server's own discovery so the tests
/// exercise the same path a user would.
fn hexaly_binary() -> Option<PathBuf> {
    discovery::locate(None).path().map(PathBuf::from)
}

fn write(directory: &Path, name: &str, source: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, source).expect("scratch directory is writable");
    path
}

/// A unique scratch directory. The wrapper must be written next to the file under diagnosis, so
/// these tests need a real directory rather than a fixture in the repository.
fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("hexaly-lsp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("temp directory is writable");
    directory
}

#[tokio::test]
async fn reports_a_syntax_error_from_the_compiler() {
    let Some(binary) = hexaly_binary() else {
        eprintln!("no Hexaly install; skipping");
        return;
    };

    let directory = scratch("syntax");
    let target = write(&directory, "model.hxm", "function model() {\n    a <- bool(;\n}\n");

    let error = hexaly::check(&binary, &target).await.expect("the file is broken");

    // Zero-based: Hexaly says line 2, LSP wants 1.
    assert_eq!(error.line, 1);
    assert!(error.message.contains("syntax error"), "got {:?}", error.message);
    assert!(error.file.ends_with("model.hxm"), "got {:?}", error.file);

    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn accepts_a_valid_model_without_a_licence() {
    let Some(binary) = hexaly_binary() else {
        eprintln!("no Hexaly install; skipping");
        return;
    };

    // The whole premise of this layer: binding declarations needs no licence, so a valid model is
    // reported clean even where solving it would be refused.
    let directory = scratch("valid");
    let target = write(
        &directory,
        "model.hxm",
        "function model() {\n    x[i in 0...5] <- bool();\n    maximize sum[i in 0...5](x[i]);\n}\n",
    );

    assert_eq!(hexaly::check(&binary, &target).await, None);

    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn catches_what_the_grammar_cannot() {
    let Some(binary) = hexaly_binary() else {
        eprintln!("no Hexaly install; skipping");
        return;
    };

    // Syntactically impeccable, so tree-sitter is silent. This is the case that justifies running
    // the compiler at all.
    let directory = scratch("duplicate");
    let target = write(
        &directory,
        "model.hxm",
        "function model() { x <- bool(); }\nfunction model() { y <- bool(); }\n",
    );

    let error = hexaly::check(&binary, &target).await.expect("duplicate declaration");
    assert!(error.message.contains("already used"), "got {:?}", error.message);

    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn resolves_sibling_modules() {
    let Some(binary) = hexaly_binary() else {
        eprintln!("no Hexaly install; skipping");
        return;
    };

    // The reason the wrapper is written beside the target rather than in a temp directory: Hexaly
    // resolves `use` against the directory of the script it was given.
    let directory = scratch("modules");
    write(&directory, "helper.hxm", "function greet() { println(\"hi\"); }\n");
    let target = write(
        &directory,
        "model.hxm",
        "use helper;\nfunction model() { x <- bool(); }\n",
    );

    assert_eq!(hexaly::check(&binary, &target).await, None);

    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn removes_its_wrapper_afterwards() {
    let Some(binary) = hexaly_binary() else {
        eprintln!("no Hexaly install; skipping");
        return;
    };

    let directory = scratch("cleanup");
    let target = write(&directory, "model.hxm", "function model() {\n    a <- bool(;\n}\n");

    hexaly::check(&binary, &target).await;

    let wrapper = hexaly::wrapper_path(&target).expect("target has a parent");
    assert!(
        !wrapper.exists(),
        "the wrapper outlived the check: {}",
        wrapper.display()
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn skips_filenames_that_are_not_valid_module_names() {
    let Some(binary) = hexaly_binary() else {
        eprintln!("no Hexaly install; skipping");
        return;
    };

    // A module name is the filename, and must be a Hexaly identifier. `use my-model;` is a syntax
    // error in the wrapper, so reporting it would blame the user's file for our own construction.
    let directory = scratch("badname");
    let target = write(&directory, "my-model.hxm", "function model() {\n    a <- bool(;\n}\n");

    assert_eq!(hexaly::check(&binary, &target).await, None);

    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn missing_binary_yields_no_diagnostics() {
    let directory = scratch("nobinary");
    let target = write(&directory, "model.hxm", "function model() {\n    a <- bool(;\n}\n");

    // Not an error: the syntax layer still works, so a machine without Hexaly degrades rather than
    // failing.
    assert_eq!(hexaly::check(Path::new("/nonexistent/hexaly"), &target).await, None);

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_configured_path_wins_and_is_not_second_guessed() {
    let configured = PathBuf::from("/somewhere/else/hexaly");
    let discovered = discovery::locate(Some(configured.clone()));

    // Reported as configured even though it does not exist: falling back would hide the user's own
    // typo behind a different Hexaly than they asked for.
    assert_eq!(discovered, Discovery::Configured(configured.clone()));
    assert_eq!(discovered.path(), Some(configured.as_path()));
}

#[test]
fn absence_is_described_rather_than_hidden() {
    let missing = Discovery::Missing;
    assert_eq!(missing.path(), None);
    assert!(missing.describe().contains("syntax diagnostics only"));
}

#[tokio::test]
async fn attributes_a_dependency_failure_to_the_use_statement() {
    let Some(binary) = hexaly_binary() else {
        eprintln!("no Hexaly install; skipping");
        return;
    };

    // The target is fine; the module it imports does not exist. Hexaly nests one frame per `use`
    // hop, so the innermost frame names the target and the message names the missing module - which
    // is what lets the server put the squiggle on the `use` line's module path.
    let directory = scratch("missingdep");
    let target = write(
        &directory,
        "model.hxm",
        "use nosuchmodule;\nfunction model() { x <- bool(); }\n",
    );

    let error = hexaly::check(&binary, &target).await.expect("unresolvable module");

    assert_eq!(error.line, 0, "the `use` is on the first line");
    assert_eq!(hexaly::failed_module(&error.message), Some("nosuchmodule"));

    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn ignores_an_error_that_belongs_to_a_dependency() {
    let Some(binary) = hexaly_binary() else {
        eprintln!("no Hexaly install; skipping");
        return;
    };

    // The break is in the dependency, so the innermost frame names `broken.hxm`, not the file under
    // diagnosis. Reporting it here would put a squiggle on a line of `model.hxm` that is correct.
    let directory = scratch("depbroken");
    write(&directory, "broken.hxm", "function helper() {\n    a <- bool(;\n}\n");
    let target = write(
        &directory,
        "model.hxm",
        "use broken;\nfunction model() { x <- bool(); }\n",
    );

    assert_eq!(
        hexaly::check(&binary, &target).await,
        None,
        "an error inside a dependency must not be misattributed to this file"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn discovery_finds_a_local_install_when_one_exists() {
    let discovered = discovery::locate(None);

    // On this machine Hexaly is on PATH via a symlink and installed under /opt, so either answer is
    // correct; what matters is that an install is not missed.
    if Path::new("/opt")
        .read_dir()
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("hexaly_"))
        })
    {
        assert_ne!(discovered, Discovery::Missing, "an install under /opt was not found");
    }
}

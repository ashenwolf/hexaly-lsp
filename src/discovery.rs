//! Locating the Hexaly installation.
//!
//! Discovery lives in the server rather than in an editor extension. Zed offers
//! `worktree.which()`, which would work and would also make this server useless in Neovim, Helix
//! and Emacs; doing it here keeps every client equal and needing no more than a `cmd`.
//!
//! Absence is a normal outcome, not an error: the syntax layer is useful on a machine with no
//! Hexaly at all, so a failed lookup degrades the server rather than failing it.

use std::path::{Path, PathBuf};

/// Where the Hexaly executable came from. Reported to the client once at startup, because "why do
/// I have no compiler diagnostics" is otherwise a question with no visible answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    Configured(PathBuf),
    OnPath(PathBuf),
    Installed(PathBuf),
    Missing,
}

impl Discovery {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Configured(path) | Self::OnPath(path) | Self::Installed(path) => Some(path),
            Self::Missing => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Configured(path) => format!("Hexaly configured at {}", path.display()),
            Self::OnPath(path) => format!("Hexaly found on PATH at {}", path.display()),
            Self::Installed(path) => format!("Hexaly found at {}", path.display()),
            Self::Missing => "Hexaly not found; syntax diagnostics only".to_string(),
        }
    }
}

/// Configured path first so a user can always override, then PATH, then the versioned install
/// directories the installers create.
pub fn locate(configured: Option<PathBuf>) -> Discovery {
    if let Some(path) = configured {
        // A configured path that does not exist is still reported as configured: silently falling
        // back would hide the user's own typo behind a different Hexaly than they asked for.
        return Discovery::Configured(path);
    }

    if let Some(path) = on_path("hexaly") {
        return Discovery::OnPath(path);
    }

    installed().map_or(Discovery::Missing, Discovery::Installed)
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

/// The newest `hexaly_*` install. The installers version the directory (`/opt/hexaly_14_0`) and
/// note that several versions coexist, so this picks one deterministically instead of pinning a
/// version that ages out.
///
/// Sorting is lexicographic over the directory name, which is right for the `major_minor` scheme
/// in use and wrong the day a two-digit minor appears (`14_10` sorts before `14_9`). Accepted for
/// now because the alternative is parsing a version scheme Hexaly has not documented as stable.
fn installed() -> Option<PathBuf> {
    let roots = ["/opt", "/usr/local", "C:\\Program Files"];

    let mut candidates: Vec<PathBuf> = roots
        .iter()
        .flat_map(|root| std::fs::read_dir(root).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase().starts_with("hexaly_"))
        })
        .flat_map(|path| [path.join("bin").join("hexaly"), path.join("bin").join("hexaly.exe")])
        .filter(|candidate| is_executable(candidate))
        .collect();

    candidates.sort();
    candidates.pop()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

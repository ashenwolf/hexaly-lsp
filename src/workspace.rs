//! Cross-file module resolution.
//!
//! This is what makes completion useful on a real model. In the 12-file production model this was
//! built against, almost none of the dotted access is standard library: it is `fn.listContains`,
//! `timeUtils.toMinutes`, `inputs.numIntervals` — module aliases pointing at the project's own
//! files. Resolving those needs no type inference at all, just name resolution, which is why it
//! buys more than the type inference it superficially resembles.
//!
//! # Resolution is entry-point-relative
//!
//! Verified against the compiler rather than assumed, because the intuitive answer is wrong.
//! `use utils.helpers;` resolves to `utils/helpers.hxm` under the directory of the *entry point*,
//! not the directory of the importing file. A file at `sub/deep.hxm` saying `use helpers;` fails
//! even when `utils/helpers.hxm` exists, and `use utils.helpers;` from that same nested file
//! succeeds. So a resolver that walked relative to the importing file would be wrong in both
//! directions.
//!
//! Since the server does not know which file is the entry point, it searches the workspace roots
//! and any ancestor directory of the importing file. That is broader than Hexaly itself, and
//! deliberately: over-resolving offers a candidate list that might not be reachable at runtime,
//! while under-resolving hides the module the user is actually working with.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::document::Document;
use crate::symbols::{self, Local, LocalKind};

/// Modules parsed from disk, keyed by path.
///
/// Cached because completion runs per keystroke and a module is parsed once per edit session, not
/// once per candidate list. Invalidation is by modification time: cheaper than watching files, and
/// the failure mode of a missed change is a stale candidate list rather than a wrong diagnostic.
#[derive(Default)]
pub struct Workspace {
    roots: Vec<PathBuf>,
    modules: HashMap<PathBuf, Module>,
}

struct Module {
    exports: Vec<Local>,
    modified: Option<std::time::SystemTime>,
}

impl Workspace {
    pub fn set_roots(&mut self, roots: Vec<PathBuf>) {
        self.roots = roots;
    }

    /// Everything a resolved module declares, for completion after `alias.`.
    ///
    /// `parser` is threaded through rather than held, because a `tree_sitter::Parser` is stateful
    /// and the server already owns one under its lock.
    pub fn exports(
        &mut self,
        parser: &mut tree_sitter::Parser,
        importer: &Path,
        module_path: &str,
    ) -> Option<&[Local]> {
        let path = self.resolve(importer, module_path)?;
        self.load(parser, &path).map(|module| module.exports.as_slice())
    }

    /// Every declaration in every `.hxm` file under the workspace roots, for `workspace/symbol`.
    ///
    /// This is the half of the outline work that `outline.scm` cannot do at all: jumping to a
    /// declaration in one of a dozen files without opening them. The production model this targets
    /// spreads ~90 functions across 12 files, which is exactly the scale where remembering *which*
    /// file something is in stops being free.
    ///
    /// Matching is a case-insensitive substring, which is what editors expect from a symbol prompt
    /// and cheap enough at this scale that no index needs maintaining between calls.
    pub fn symbols(&mut self, parser: &mut tree_sitter::Parser, query: &str) -> Vec<(PathBuf, Local)> {
        let needle = query.to_lowercase();

        self.hxm_files()
            .into_iter()
            .flat_map(|path| {
                let declarations = self
                    .load(parser, &path)
                    .map(|module| module.exports.clone())
                    .unwrap_or_default();

                declarations
                    .into_iter()
                    .filter(|local| needle.is_empty() || local.name.to_lowercase().contains(&needle))
                    .map(move |local| (path.clone(), local))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Every `.hxm` file under the roots.
    ///
    /// Depth is capped rather than unbounded: a workspace root can be a whole monorepo, and walking
    /// it on every symbol query would make the feature feel broken on exactly the large projects
    /// where it is most wanted.
    fn hxm_files(&self) -> Vec<PathBuf> {
        const MAX_DEPTH: usize = 6;

        let mut found = Vec::new();
        let mut queue: Vec<(PathBuf, usize)> = self.roots.iter().cloned().map(|root| (root, 0)).collect();

        while let Some((directory, depth)) = queue.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };

            for entry in entries.flatten() {
                let path = entry.path();

                if path.is_dir() {
                    // Skipped by name: these hold build output and dependencies, never a model the
                    // user would navigate to, and they are where the file count explodes.
                    let skip = path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
                        name.starts_with('.') || matches!(name, "build" | "target" | "node_modules" | "env")
                    });

                    if !skip && depth < MAX_DEPTH {
                        queue.push((path, depth + 1));
                    }
                } else if path.extension().is_some_and(|extension| extension == "hxm") {
                    found.push(path);
                }
            }
        }

        found.sort();
        found
    }

    /// The members of a class imported from another file, for `use Name from path;`.
    ///
    /// One level deeper than `exports`: that returns a module's own declarations, this looks inside
    /// the module for a named class and returns *its* members. `SpecialLocations.PICK_MANUAL` needs
    /// this, and treating the import as a module would instead offer the class as its own member.
    pub fn imported_members(
        &mut self,
        parser: &mut tree_sitter::Parser,
        importer: &Path,
        module_path: &str,
        name: &str,
    ) -> Option<Vec<Local>> {
        let path = self.resolve(importer, module_path)?;
        let text = std::fs::read_to_string(&path).ok()?;
        let document = Document::open(parser, text, 0);

        let members = symbols::class_members(&document, name);
        (!members.is_empty()).then_some(members)
    }

    /// The file a dotted module path names, if one exists.
    ///
    /// Candidate roots are the workspace roots plus every ancestor of the importing file, nearest
    /// first: a model is usually opened at or below its own directory, so the nearest ancestor that
    /// contains the module is the one Hexaly would have used as the entry point's directory.
    pub fn resolve(&self, importer: &Path, module_path: &str) -> Option<PathBuf> {
        let relative = PathBuf::from(format!("{}.hxm", module_path.replace('.', "/")));

        importer
            .ancestors()
            .skip(1)
            .map(Path::to_path_buf)
            .chain(self.roots.iter().cloned())
            .map(|root| root.join(&relative))
            .find(|candidate| candidate.is_file())
    }

    fn load(&mut self, parser: &mut tree_sitter::Parser, path: &Path) -> Option<&Module> {
        let modified = std::fs::metadata(path).ok().and_then(|data| data.modified().ok());

        let stale = self.modules.get(path).is_none_or(|module| module.modified != modified);

        if stale {
            let text = std::fs::read_to_string(path).ok()?;
            let document = Document::open(parser, text, 0);

            self.modules.insert(
                path.to_path_buf(),
                Module {
                    exports: exported(&document),
                    modified,
                },
            );
        }

        self.modules.get(path)
    }
}

/// What a module offers to an importer.
///
/// Only top-level declarations. `symbols::locals` walks the whole tree, which is right for
/// completing inside the file being edited - a local four lines up is exactly what you want - but
/// wrong across a module boundary: a loop counter inside one of `fn`'s functions is not reachable as
/// `fn.i`, and offering it is a suggestion that cannot compile.
///
/// Neither `use` form is re-exported. Importing a module does not make its imports reachable through
/// it, so neither `fn.io` (a module it imports) nor `fn.SpecialLocations` (a member it imports) is a
/// path that exists.
fn exported(document: &Document) -> Vec<Local> {
    symbols::top_level(document)
        .into_iter()
        .filter(|local| !matches!(local.kind, LocalKind::Module | LocalKind::Import))
        .collect()
}

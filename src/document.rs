//! Document state: the text of an open buffer and its parse tree, kept in step.
//!
//! Two coordinate systems meet here. LSP positions are (line, UTF-16 code unit) pairs; Rust
//! strings and tree-sitter are byte-oriented. Nothing else in the server should have to think
//! about that, so every conversion lives in this module.

use tower_lsp_server::ls_types::{Position, Range, TextDocumentContentChangeEvent};

/// An open buffer and its tree. The tree is reused across edits: tree-sitter reparses only the
/// span that changed, which is what makes per-keystroke diagnostics affordable.
pub struct Document {
    text: String,
    tree: tree_sitter::Tree,
    version: i32,
}

impl Document {
    pub fn open(parser: &mut tree_sitter::Parser, text: String, version: i32) -> Self {
        let tree = parse(parser, &text, None);
        Self { text, tree, version }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn tree(&self) -> &tree_sitter::Tree {
        &self.tree
    }

    pub fn version(&self) -> i32 {
        self.version
    }

    /// Applies one `didChange` batch. Changes are applied in order and each is reported to the
    /// tree before reparsing, so the reparse can reuse everything the edits did not touch.
    ///
    /// A change without a range means a full replacement, which is what a client sends under
    /// `TextDocumentSyncKind::FULL` or when it gives up on incremental sync mid-session.
    pub fn apply(
        &mut self,
        parser: &mut tree_sitter::Parser,
        changes: Vec<TextDocumentContentChangeEvent>,
        version: i32,
    ) {
        for change in changes {
            match change.range {
                Some(range) => self.splice(parser, range, &change.text),
                None => {
                    self.text = change.text;
                    // A whole-document replacement shares nothing with the old tree, so reusing
                    // it would cost more than it saves.
                    self.tree = parse(parser, &self.text, None);
                }
            }
        }

        self.version = version;
    }

    fn splice(&mut self, parser: &mut tree_sitter::Parser, range: Range, replacement: &str) {
        let Some(start) = self.byte_offset(range.start) else {
            return;
        };
        let Some(end) = self.byte_offset(range.end) else {
            return;
        };

        // The edit is described to tree-sitter in terms of the OLD text, so the start and old-end
        // points are computed before the splice. The new-end point is computed after, against the
        // new text, because that is the coordinate system it belongs to.
        let start_position = self.point(start);
        let old_end_position = self.point(end);

        self.text.replace_range(start..end, replacement);

        let new_end_byte = start + replacement.len();

        self.tree.edit(&tree_sitter::InputEdit {
            start_byte: start,
            old_end_byte: end,
            new_end_byte,
            // tree-sitter wants point coordinates in bytes-within-line, matching the byte offsets
            // above rather than the UTF-16 units the LSP range arrived as.
            start_position,
            old_end_position,
            new_end_position: self.point(new_end_byte),
        });

        self.tree = parse(parser, &self.text, Some(&self.tree));
    }

    /// Byte offset of an LSP position, or `None` when the client names a position outside the
    /// document. Out-of-range is not worth panicking over: it happens when a change batch is
    /// computed against a version we have already moved past.
    fn byte_offset(&self, position: Position) -> Option<usize> {
        let line_start = self
            .text
            .split_inclusive('\n')
            .take(position.line as usize)
            .map(str::len)
            .sum::<usize>();

        let line = self.text.get(line_start..)?.split_inclusive('\n').next()?;

        // Walk the line accumulating UTF-16 units until we reach the requested column. A
        // character outside the BMP counts as two units but may be up to four bytes, so the two
        // counts diverge and cannot be derived from one another.
        let mut utf16 = 0usize;
        for (offset, ch) in line.char_indices() {
            if utf16 >= position.character as usize {
                return Some(line_start + offset);
            }
            utf16 += ch.len_utf16();
        }

        Some(line_start + line.len())
    }

    fn point(&self, byte: usize) -> tree_sitter::Point {
        let before = &self.text[..byte];
        let row = before.matches('\n').count();
        let column = before.len() - before.rfind('\n').map_or(0, |index| index + 1);
        tree_sitter::Point { row, column }
    }

    /// LSP position of a tree-sitter point. The inverse of `byte_offset`, and the direction
    /// diagnostics need: tree-sitter reports where an error is in bytes, the client wants UTF-16.
    ///
    /// Getting this wrong is invisible on ASCII and misplaces every squiggle after the first
    /// non-ASCII character on a line, which is why it is tested directly.
    pub fn position(&self, point: tree_sitter::Point) -> Position {
        let line = self.text.split_inclusive('\n').nth(point.row).unwrap_or_default();

        // `point.column` is a byte offset within the line; clamp it because a point at
        // end-of-file can name the position just past the last line's content.
        let column = line.len().min(point.column);
        let character = line[..column].chars().map(char::len_utf16).sum::<usize>();

        Position {
            line: point.row as u32,
            character: character as u32,
        }
    }

    pub fn range(&self, node: tree_sitter::Node<'_>) -> Range {
        Range {
            start: self.position(node.start_position()),
            end: self.position(node.end_position()),
        }
    }

    /// The whole of `line`, as a range. The fallback for a compiler diagnostic: Hexaly reports no
    /// column, and a zero-width range at column 0 renders as an invisible squiggle.
    pub fn line_range(&self, line: u32) -> Range {
        let text = self.text.lines().nth(line as usize).unwrap_or_default();
        let end = text.chars().map(char::len_utf16).sum::<usize>();

        Range {
            start: Position { line, character: 0 },
            end: Position {
                line,
                character: end as u32,
            },
        }
    }

    /// Range of the `module_path` in the `use` statement naming `module`, if the file has one.
    ///
    /// This is what upgrades an unresolvable-module error from a whole line to the offending name:
    /// the compiler says only "cannot load module 'x'" with the line of the `use`, and the tree
    /// knows where within that line the name sits.
    pub fn module_path_range(&self, module: &str) -> Option<Range> {
        let mut cursor = self.tree.walk();
        let root = self.tree.root_node();

        root.children(&mut cursor)
            .filter(|node| node.kind() == "use_statement")
            .collect::<Vec<_>>()
            .into_iter()
            .find_map(|statement| {
                let path = statement.child_by_field_name("module")?;
                // Compared against the source text rather than a reconstructed name so a dotted
                // path (`use utils.fn;`) matches exactly as Hexaly spelled it in the message.
                (path.utf8_text(self.text.as_bytes()).ok()? == module).then(|| self.range(path))
            })
    }
}

/// `Parser::parse` is fallible only when a timeout or cancellation flag is set, and this server
/// sets neither: a parse that cannot fail should not push an `Option` through every caller.
fn parse(parser: &mut tree_sitter::Parser, text: &str, old: Option<&tree_sitter::Tree>) -> tree_sitter::Tree {
    parser
        .parse(text, old)
        .expect("the Hexaly language is set and no cancellation flag is configured")
}

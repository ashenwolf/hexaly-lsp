//! Formatting: fixing indentation and spacing without rewriting the author's layout.
//!
//! This is deliberately a *normaliser*, not a pretty-printer. A pretty-printer parses to a tree,
//! discards the original layout and re-emits from scratch, which produces better output and fails
//! catastrophically: any gap in the grammar becomes deleted or mangled code. This grammar has been
//! wrong three times in this project's short life, and a formatter is the one feature where being
//! wrong costs the user their work rather than an unhelpful popup.
//!
//! So line structure and blank lines are preserved exactly, and only two things change: the leading
//! whitespace of each line, and the spacing between tokens within it. Everything the author decided
//! - where a statement breaks, which declarations are grouped - survives untouched.
//!
//! The conventions implemented here were measured from the 66 models Hexaly ships, not invented:
//! 4-space indent (2896 occurrences against 1346 at 8, i.e. multiples of 4), same-line open brace
//! (491 against 1), spaces around binary operators (46 spaced `<=`, 0 unspaced), and `, ` between
//! arguments (653 against 8). Where the corpus is silent, the input is left alone.

use crate::token::{self, Tokens};

const INDENT: &str = "    ";

/// Formats `text`, or returns `None` if it is already formatted or the result cannot be trusted.
///
/// `None` rather than an unchanged copy, because the LSP answer to "format this" is an edit list,
/// and an empty list is the honest encoding of "nothing to do" - it also stops an editor marking the
/// buffer dirty on a no-op format.
pub fn format(text: &str) -> Option<String> {
    let formatted = normalise(text);

    if formatted == text {
        return None;
    }

    // The guard that makes this safe to run on save. A formatter is only allowed to move
    // whitespace; if the token sequence changed, something was rewritten, and returning nothing at
    // all is strictly better than returning damage. This turns "I believe the rules are
    // conservative" into a property checked on every single format.
    token::equivalent(text, &formatted).then_some(formatted)
}

/// The formatting rules, without the safety check.
fn normalise(text: &str) -> String {
    let mut depth = 0usize;
    let mut lines = Vec::new();
    // Whether the previous line left an expression open, either inside a bracket or as an unfinished
    // statement. Such a line's indentation is left exactly as the author wrote it - see below.
    let mut open_brackets = 0usize;
    let mut unfinished = false;

    for line in text.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            // A blank line stays blank rather than being indented to depth: trailing whitespace on
            // an empty line is what most formatters are configured to strip.
            lines.push(String::new());
            continue;
        }

        let nesting = token::Nesting::of(trimmed);

        // A closing brace belongs to the enclosing block, so it dedents before being written rather
        // than after - otherwise every block's closer sits one level too deep.
        depth = depth.saturating_sub(nesting.leading_close_braces);

        let continuation = open_brackets > 0 || unfinished;
        let spaced = spacing(trimmed, continuation);

        // A continuation line keeps the author's own indentation. Hexaly's models align these by hand
        // to whatever the wrapped expression makes readable - measured across the corpus, the indent
        // relative to the line above is 0 in 191 cases and 8 in 67, with a plain +4 the rarest at 13.
        // There is no rule to apply, so applying one would only fight the author.
        if continuation {
            let indent = &line[..line.len() - line.trim_start().len()];
            lines.push(format!("{indent}{spaced}"));
        } else {
            lines.push(format!("{}{spaced}", INDENT.repeat(depth)));
        }

        // Only the braces this line opens and closes *itself* move the depth for the next line; the
        // leading closers were already applied above.
        depth = (depth + nesting.open_braces)
            .saturating_sub(nesting.close_braces.saturating_sub(nesting.leading_close_braces));
        open_brackets = open_brackets
            .saturating_add(nesting.open_brackets)
            .saturating_sub(nesting.close_brackets);
        unfinished = nesting.continues_a_statement;
    }

    let mut formatted = lines.join("\n");

    // A trailing newline is restored only if the input had one: adding one to a file that lacks it
    // is a change to content, not layout, and removing one would fight every other tool.
    if text.ends_with('\n') {
        formatted.push('\n');
    }

    formatted
}

/// Normalises the spacing *within* one already-trimmed line.
///
/// Works token by token from the tokenizer, so a string literal or comment is copied verbatim and
/// nothing inside it can be reformatted - the single most important property here, since `"a,b"` and
/// `// x  =  1` must survive exactly.
///
/// `continuation` says this line sits inside an unclosed bracket, which the line's own tokens cannot
/// reveal. Without it, a wrapped sum beginning `+ cost[i]` looks like a leading sign and gets glued
/// to its operand.
fn spacing(line: &str, continuation: bool) -> String {
    let tokens = Tokens::new(line).collect::<Vec<_>>();
    let mut out = String::with_capacity(line.len());

    for (index, current) in tokens.iter().enumerate() {
        let Some(previous) = index.checked_sub(1).and_then(|before| tokens.get(before)) else {
            out.push_str(current.text);
            continue;
        };

        // Three tokens, not two: telling `a - b` from `f(-5)` needs what precedes the sign, since a
        // sign is unary exactly when there is no left operand for it to bind.
        let preceding = index.checked_sub(2).and_then(|before| tokens.get(before));

        // On a continuation line the left operand is on an earlier line, so the first operator must
        // be treated as binary. `token::CONTINUED` stands in for that off-line operand.
        let preceding = match (index, continuation) {
            (1, true) => Some(&token::CONTINUED),
            _ => preceding,
        };

        if token::needs_space(preceding, previous, current) {
            out.push(' ');
        }

        out.push_str(current.text);
    }

    out
}

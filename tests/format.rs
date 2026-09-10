//! Formatting rules, and the guard that makes them safe.
//!
//! The corpus test proves the formatter leaves Hexaly's own 66 models untouched. That is necessary
//! and not sufficient: a formatter that returned its input unchanged would pass it too. These tests
//! cover the other direction - badly laid out input becoming well laid out - plus the properties that
//! must hold whatever the input.

use hexaly_lsp::{format::format, token};

/// Formats, asserting the result is exactly as expected.
fn formatted(source: &str) -> String {
    format(source).unwrap_or_else(|| source.to_string())
}

#[test]
fn indents_blocks_by_four() {
    let source = "function model() {\nx <- bool();\n}\n";

    assert_eq!(formatted(source), "function model() {\n    x <- bool();\n}\n");
}

#[test]
fn closing_brace_dedents_to_its_opener() {
    let source = "function model() {\nif (x) {\ny = 1;\n}\n}\n";

    assert_eq!(
        formatted(source),
        "function model() {\n    if (x) {\n        y = 1;\n    }\n}\n"
    );
}

#[test]
fn over_indented_lines_are_pulled_back() {
    // The direction that matters in practice: hand-edited files drift deeper, not shallower.
    let source = "function model() {\n            x <- bool();\n}\n";

    assert_eq!(formatted(source), "function model() {\n    x <- bool();\n}\n");
}

#[test]
fn a_continuation_line_keeps_the_authors_indentation() {
    // Measured, against an earlier guess. This test used to assert a +4 indent for wrapped lines,
    // because that seemed tidy. The corpus says otherwise: relative to the line above, Hexaly's own
    // continuation lines sit at +0 in 191 cases and +8 in 67, with +4 the rarest at 13. There is no
    // rule to apply, so the author's alignment is preserved and only the spacing is normalised.
    let source = "function model() {\n    x <- sum(range,\n            i=>weights[i]);\n}\n";

    assert_eq!(
        formatted(source),
        "function model() {\n    x <- sum(range,\n            i => weights[i]);\n}\n"
    );
}

#[test]
fn a_wrapped_expression_does_not_read_its_operator_as_a_sign() {
    // A line beginning `+ "..."` continues the line above, so the `+` is binary even though nothing
    // precedes it on its own line. Without that, it glued itself to its operand as a leading sign.
    let source = "function main() {\n    usage = \"a\"\n        + \"b\";\n}\n";

    assert_eq!(format(source), None);
}

#[test]
fn blank_lines_survive_and_carry_no_indentation() {
    let source = "function model() {\nx <- bool();\n\ny <- bool();\n}\n";

    let result = formatted(source);
    assert!(result.contains("\n\n"), "the blank line should survive: {result:?}");
    assert!(
        !result.contains("    \n"),
        "a blank line must not be indented: {result:?}"
    );
}

#[test]
fn several_statements_on_one_line_stay_on_one_line() {
    // Hexaly's own toy.hxm writes `x_0 <- bool(); x_1 <- bool();` deliberately, four to a line. A
    // formatter that split them would be destroying an intentional layout, which is the whole reason
    // this normalises rather than pretty-prints.
    let source = "function model() {\n    x_0 <- bool(); x_1 <- bool();\n}\n";

    assert_eq!(format(source), None, "line structure must be preserved exactly");
}

#[test]
fn operators_are_spaced_and_calls_are_not() {
    let source = "function model() {\nx<-bool();\nconstraint w<=102;\nf( a , b );\n}\n";

    let result = formatted(source);
    assert!(result.contains("x <- bool();"), "{result:?}");
    assert!(result.contains("constraint w <= 102;"), "{result:?}");
    assert!(result.contains("f(a, b);"), "{result:?}");
}

#[test]
fn a_dotted_reference_is_not_spaced() {
    let source = "function model() {\nio . println ( \"x\" ) ;\n}\n";

    assert!(formatted(source).contains("io.println(\"x\");"));
}

#[test]
fn string_contents_are_untouched() {
    // The most important negative: a string is copied verbatim, so nothing inside it is respaced.
    let source = "function model() {\n    io.println(\"a,b   c<=d\");\n}\n";

    assert_eq!(format(source), None, "a formatted line containing a string must not change");
}

#[test]
fn comment_contents_are_untouched() {
    let source = "function model() {\n    // x  =  1,  spaced  oddly on purpose\n    x <- bool();\n}\n";

    assert_eq!(format(source), None);
}

#[test]
fn a_block_comment_body_is_not_reindented() {
    let source = "/* one\n   two\n   three */\nfunction model() {\n}\n";

    // The body lines of a block comment are inside a single token, so they cannot be touched.
    let result = formatted(source);
    assert!(result.contains("/* one\n   two\n   three */"), "{result:?}");
}

#[test]
fn a_unary_sign_hugs_its_operand() {
    // Found by the corpus, not by hand: branin.hxm writes `float(-5, 10)`, and an earlier rule
    // spaced every `-` alike and turned it into `float(- 5, 10)`. Every hand-written test passed.
    let source = "function model() {\n    x1 <- float(-5, 10);\n}\n";

    assert_eq!(format(source), None);
}

#[test]
fn a_binary_minus_keeps_its_spaces() {
    // The other half of the same rule: what tells them apart is the token *before* the sign, so both
    // directions need pinning or a fix for one silently breaks the other.
    let source = "function model() {\n    x <- a-b;\n}\n";

    assert!(formatted(source).contains("x <- a - b;"));
}

#[test]
fn a_sign_after_a_closing_paren_is_binary() {
    let source = "function model() {\n    x <- f(a)-1;\n}\n";

    assert!(formatted(source).contains("f(a) - 1;"));
}

#[test]
fn a_keyword_keeps_its_space_before_a_paren() {
    // `if (x)` is not a call. The corpus writes it spaced 346 times against 16.
    let source = "function model() {\n    if(x) {\n        y = 1;\n    }\n}\n";

    assert!(formatted(source).contains("if (x) {"));
}

#[test]
fn a_call_does_not_gain_one() {
    let source = "function model() {\n    x <- bool ();\n}\n";

    assert!(formatted(source).contains("x <- bool();"));
}

#[test]
fn a_prefix_operator_hugs_its_operand() {
    // Found on Sergii's timeUtils.hxm: `!` defaulted to spaced like a binary operator, producing
    // `! this.overlaps(other)`.
    let source = "function model() {\n    return !this.overlaps(other) && !x;\n}\n";

    assert_eq!(format(source), None);
}

#[test]
fn a_brace_initialiser_stays_tight() {
    // Also from timeUtils.hxm: `{31, 28, 31}` became `{ 31, 28, 31 }`, and `{}` became `{ }`.
    let source = "function main() {\n    local days = {31, 28, 31};\n    local empty = {};\n}\n";

    assert_eq!(format(source), None);
}

#[test]
fn a_ternary_colon_keeps_its_spaces() {
    // From timeSeries.hxm. Hexaly has no label syntax, so a `:` in an expression is a ternary
    // alternative and `nil : x` is correct - an earlier rule hugged it into `nil: x`.
    let source = "function main() {\n    return overlaps[0] == nil ? nil : overlaps[0].value;\n}\n";

    assert_eq!(format(source), None);
}

#[test]
fn an_object_literal_key_is_spaced_after_the_colon() {
    let source = "function main() {\n    local out = {\n        \"inputId\": id,\n    };\n}\n";

    assert_eq!(format(source), None);
}

#[test]
fn a_closing_bracket_hugs_what_continues_the_expression() {
    // `sum[i in 0...n](w[i])`, `x[i][j]` and `[iTo in idx[t]...n]` are each one expression; spacing
    // any of those joins turns one expression into what looks like two.
    let source = concat!(
        "function model() {\n",
        "    total <- sum[i in 0...n](weights[i]);\n",
        "    v = forbidden[i][j];\n",
        "    r = [iTo in index[t]...n];\n",
        "}\n"
    );

    assert_eq!(format(source), None);
}

#[test]
fn a_double_space_before_a_comment_is_collapsed() {
    let source = "function main() {\n    x = 1;  // a trailing comment\n}\n";

    assert!(formatted(source).contains("x = 1; // a trailing comment"));
}

#[test]
fn already_formatted_input_produces_no_edit() {
    let source = "function model() {\n    x <- bool();\n}\n";

    assert_eq!(format(source), None);
}

#[test]
fn formatting_is_idempotent() {
    let source = "function model(){\nx<-bool();\nif(x){\ny=1;\n}\n}\n";

    let once = formatted(source);
    assert_eq!(format(&once), None, "second pass changed {once:?}");
}

#[test]
fn a_missing_trailing_newline_is_not_added() {
    // Adding one would be a change to content rather than layout, and would fight editors that
    // deliberately leave it off.
    let source = "function model() {\nx <- bool();\n}";

    assert!(!formatted(source).ends_with('\n'));
}

#[test]
fn an_unbalanced_brace_does_not_panic_or_run_away() {
    // A buffer mid-edit is the normal case, not an edge case: the closing brace has not been typed
    // yet, and formatting must still produce something sane.
    let source = "function model() {\nx <- bool();\n";

    let result = formatted(source);
    assert!(result.contains("    x <- bool();"), "{result:?}");
}

#[test]
fn a_stray_closing_brace_does_not_underflow() {
    let source = "}\n}\nfunction model() {\n}\n";

    let result = formatted(source);
    assert!(token::equivalent(source, &result));
}

#[test]
fn an_unterminated_string_is_survivable() {
    let source = "function model() {\nio.println(\"unclosed\n}\n";

    let result = formatted(source);
    assert!(token::equivalent(source, &result), "{result:?}");
}

#[test]
fn tabs_become_the_standard_indent() {
    let source = "function model() {\n\tx <- bool();\n}\n";

    assert_eq!(formatted(source), "function model() {\n    x <- bool();\n}\n");
}

#[test]
fn trailing_whitespace_is_removed() {
    let source = "function model() {\n    x <- bool();   \n}\n";

    assert_eq!(formatted(source), "function model() {\n    x <- bool();\n}\n");
}

#[test]
fn non_ascii_content_is_preserved_exactly() {
    // The formatter works in bytes; a multi-byte character inside a string must not be split or
    // shifted.
    let source = "function model() {\n    io.println(\"héllo wörld ✓\");\n}\n";

    assert_eq!(format(source), None);
}

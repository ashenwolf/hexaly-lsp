//! Tokenizing, for formatting and for checking that formatting changed nothing but whitespace.
//!
//! Not tree-sitter. The formatter must work on a file that does not parse - a buffer mid-edit is the
//! normal case, and refusing to format anything with a syntax error would make the feature useless
//! exactly when layout is worst. A tokenizer degrades gracefully where a parser cannot: an unknown
//! character becomes a token of its own and is copied through.
//!
//! It exists in service of one property: a formatter may move whitespace and nothing else. Comparing
//! the token sequence before and after is how that is enforced rather than hoped for.

/// A token's kind, at the granularity the spacing rules actually distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An identifier, keyword or number. Spacing does not need to tell these apart: what matters is
    /// that they are a run of word characters, and two of them are always separated.
    Word,
    /// A string literal, copied verbatim.
    String,
    /// A comment, copied verbatim.
    Comment,
    /// An operator or punctuation mark.
    Operator,
}

/// One token: its kind, its exact source text, and whether whitespace preceded it.
///
/// `spaced_before` records what the author wrote, so a rule can decline to have an opinion and
/// preserve it. Deliberately excluded from `equivalent`, which compares only kind and text: the whole
/// point of the formatter is that spacing may change, so including it would make the guard vacuous.
#[derive(Debug, Clone, Copy)]
pub struct Token<'a> {
    pub kind: Kind,
    pub text: &'a str,
    pub spaced_before: bool,
}

/// Stands in for an operand on a previous line.
///
/// A continuation line's first operator has its left operand out of sight, on an earlier line. Passing
/// this as the preceding token says "something that can be an operand is to the left", which is what
/// stops a wrapped `+ cost[i]` being read as a leading sign.
pub const CONTINUED: Token<'static> = Token {
    kind: Kind::Word,
    text: "",
    spaced_before: true,
};

/// Tokens of a source string, whitespace discarded.
pub struct Tokens<'a> {
    source: &'a str,
    offset: usize,
}

impl<'a> Tokens<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { source, offset: 0 }
    }
}

/// What one line's tokens do to nesting.
///
/// Computed in a single pass rather than by asking the tokenizer four separate questions: the counts
/// are always wanted together, and one pass cannot disagree with itself about where a string ends.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Nesting {
    /// `{` opened on this line.
    pub open_braces: usize,
    /// `}` closed on this line.
    pub close_braces: usize,
    /// `}` appearing before any other token, which close blocks from earlier lines and so must
    /// dedent the line they sit on - unlike a `}` later in the line, which closes something opened
    /// on the same line.
    pub leading_close_braces: usize,
    /// `(` and `[` opened on this line.
    pub open_brackets: usize,
    /// `)` and `]` closed on this line.
    pub close_brackets: usize,
    /// Whether this line leaves a statement unfinished, so the next line continues it.
    ///
    /// True when the line's last token is neither a terminator (`;`), a brace, nor a comment - the
    /// corpus wraps long concatenations as `usage = "..."` then `+ "..."`, where no bracket is open
    /// but the expression obviously continues.
    pub continues_a_statement: bool,
}

impl Nesting {
    pub fn of(line: &str) -> Self {
        let mut nesting = Self::default();
        let mut only_closers_so_far = true;
        let mut last_code_token = None;

        for token in Tokens::new(line) {
            if token.kind != Kind::Comment {
                last_code_token = Some(token);
            }

            if token.kind != Kind::Operator {
                only_closers_so_far = false;
                continue;
            }

            match token.text {
                "{" => nesting.open_braces += 1,
                "}" => {
                    nesting.close_braces += 1;
                    if only_closers_so_far {
                        nesting.leading_close_braces += 1;
                    }
                }
                "(" | "[" => nesting.open_brackets += 1,
                ")" | "]" => nesting.close_brackets += 1,
                _ => {}
            }

            if token.text != "}" {
                only_closers_so_far = false;
            }
        }

        nesting.continues_a_statement =
            last_code_token.is_some_and(|token| !matches!(token.text, ";" | "{" | "}" | ":"));

        nesting
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let rest = self.source.get(self.offset..)?;
        let trimmed = rest.trim_start();
        let skipped = rest.len() - trimmed.len();
        self.offset += skipped;

        let mut characters = trimmed.char_indices();
        let (_, first) = characters.next()?;

        let (kind, length) = match first {
            '"' => (Kind::String, string_length(trimmed)),
            // A line comment ends at the newline, not at the end of the input. Getting this wrong
            // made the whole-file tokenizer return a single token containing everything after the
            // first comment - which silently disabled the formatter's safety guard, since comparing
            // one giant token to another always agreed.
            '/' if trimmed.starts_with("//") => (Kind::Comment, line_comment_length(trimmed)),
            '/' if trimmed.starts_with("/*") => (Kind::Comment, block_comment_length(trimmed)),
            character if is_word(character) => (Kind::Word, word_length(trimmed)),
            // Multi-character operators are matched longest-first so `<-` does not become `<` then
            // `-`, which would change both spacing and the token sequence.
            _ => (Kind::Operator, operator_length(trimmed)),
        };

        let text = trimmed.get(..length)?;
        self.offset += length;
        Some(Token {
            kind,
            text,
            spaced_before: skipped > 0,
        })
    }
}

/// Whether two adjacent tokens need a space between them.
///
/// `preceding` is the token before `previous`, needed only to tell a unary sign from a binary
/// operator: `-` is a sign exactly when nothing to its left could be a left operand.
///
/// Expressed as a table of the cases that must be separated or must not be, with "leave a single
/// space" as the default. That ordering matters: an operator this list has never heard of gets
/// spaced like a binary operator, which is the common case and is what the Hexaly corpus does.
pub fn needs_space(preceding: Option<&Token<'_>>, previous: &Token<'_>, current: &Token<'_>) -> bool {
    use Kind::{Comment, Operator, String as Str, Word};

    match (previous.kind, current.kind) {
        // Two words are always separated - `constraint x`, `function main`. Without this, keywords
        // would fuse with what follows them.
        (Word | Str, Word | Str) => true,

        // Nothing goes between a name and the parenthesis or subscript that follows it: `bool()`,
        // `weights[i]`, never `bool ()`. Nor before punctuation that closes or separates.
        //
        // A keyword is the exception, because `if (x)` is not a call: the corpus writes `if (` 346
        // times against 16, and `while (` 14 against 5.
        (Word | Str, Operator) => {
            // A keyword takes a space before a bracket, because `if (x)` and `for [i in ...]` are
            // clauses rather than a call or a subscript. Both bracket kinds matter: Hexaly writes its
            // index spaces as `for [i in 0...n]`.
            if matches!(current.text, "(" | "[") {
                return is_keyword(previous.text);
            }

            // A multiplicative operator's left-hand spacing is the author's, read from the operator
            // itself - the token on that side of the gap.
            if is_multiplicative(current.text) {
                return current.spaced_before;
            }

            // `:` appears in two forms with different shapes, and both appear in real models: an
            // object-literal key hugs it (`"inputId": id`), a ternary alternative is spaced from it
            // (`cond ? a : b`). Neither can be inferred from the two adjacent tokens, so the author's
            // own spacing decides - which is right for both and changes neither.
            if current.text == ":" {
                return current.spaced_before;
            }

            !matches!(current.text, ")" | "]" | "," | ";" | "." | "..." | ".." | "}")
        }

        // An opening delimiter or a dot hugs what follows: `(x`, `[i`, `io.openRead`. So does a
        // unary sign - `float(-5, 10)` in branin.hxm is where this was found, having passed every
        // hand-written test - and so does the range operator, which the corpus writes tight
        // (`0...nbItems`) 1038 times.
        (Operator, Word | Str) => {
            // A multiplicative operator's right-hand spacing is the author's - see
            // `is_multiplicative`. Read from `current`, the token on that side, so the two sides stay
            // independent and a second pass finds nothing to change.
            if is_multiplicative(previous.text) {
                return current.spaced_before;
            }

            // A prefix operator hugs its operand: `!this.overlaps(other)`.
            if is_prefix(previous.text) {
                return false;
            }

            !matches!(previous.text, "(" | "[" | "." | "..." | ".." | "{")
                && !(is_sign(previous.text) && is_unary(preceding))
        }

        (Operator, Operator) => match (previous.text, current.text) {
            // Punctuation that closes or separates never has space before it, whatever precedes it.
            (_, ")" | "]" | "," | ";" | ".") => false,
            // An opening delimiter, dot or range hugs what follows, including another delimiter.
            ("(" | "[" | "." | "..." | "..", _) => false,
            // A closing bracket hugs a bracket or a range that follows it: `sum[i in 0...n](w[i])`,
            // `forbidden[i][j]` and `[iTo in idx[t]...n]` are each one continuous expression.
            (")" | "]", "(" | "[" | "..." | "..") => false,
            // A unary sign hugs a parenthesised operand too: `-(a + b)`.
            (sign, "(") if is_sign(sign) && is_unary(preceding) => false,
            // Multiplicative operators keep the author's spacing on each side independently, each
            // read from the token that sits on that side of the gap.
            (multiplicative, _) if is_multiplicative(multiplicative) => current.spaced_before,
            (_, multiplicative) if is_multiplicative(multiplicative) => current.spaced_before,
            // A prefix operator hugs what follows, including a parenthesised operand: `!(a && b)`.
            (prefix, _) if is_prefix(prefix) => false,
            // A brace initialiser is written tight in the corpus and in real models: `{}`,
            // `{31, 28, 31}`. Spacing it to `{ }` was a regression found on Sergii's timeUtils.hxm.
            ("{", _) | (_, "}") => false,
            // Everything else - `) {`, `= -`, `<- bool` - takes a single space. Defaulting to spaced
            // is deliberate: an operator this code has never seen behaves like a binary operator,
            // which is both the common case and what the Hexaly corpus does.
            _ => true,
        },

        // A comment keeps one space from the code beside it and is otherwise untouched.
        (_, Comment) | (Comment, _) => true,
    }
}

/// Multiplicative operators, whose spacing the formatter does not touch.
///
/// The corpus is split and for a good reason: `7*pow(x1, 3)/3` in a dense polynomial reads better
/// tight, `5.1 / (4 * pow(PI, 2))` reads better spaced, and both appear in Hexaly's own models. `*` is
/// spaced 70 times against 9, `/` is 12 against 15.
///
/// Two earlier attempts got this wrong in instructive ways. Normalising *tight* rewrote clean spaced
/// arithmetic. Preserving one side only read `spaced_before` of the left operand, which turned
/// `)/3` into `) /3` and then `) / 3` on the next pass - not idempotent, caught by the corpus test.
///
/// So each side is preserved independently, from that side's own token. The formatter has no opinion
/// here, which is the honest position when the reference corpus has none either.
fn is_multiplicative(text: &str) -> bool {
    matches!(text, "*" | "/" | "%" | "^")
}

/// Prefix operators, which hug their operand.
///
/// `!this.overlaps(other)` and `~mask` are one expression, not two: an earlier version defaulted these
/// to spaced like a binary operator and produced `! this.overlaps(other)` in Sergii's own model.
fn is_prefix(text: &str) -> bool {
    matches!(text, "!" | "~")
}

/// Whether an operator can be a sign as well as a binary operator.
fn is_sign(text: &str) -> bool {
    matches!(text, "-" | "+")
}

/// Whether a sign preceded by `preceding` is unary.
///
/// A sign is unary when there is no left operand for it to bind: at the start of a line, or after an
/// opening delimiter, a separator, or another operator. It is binary after a word, a string, or a
/// closing delimiter - the three things that can *be* a left operand.
fn is_unary(preceding: Option<&Token<'_>>) -> bool {
    match preceding {
        // Nothing to the left at all, as in a continuation line beginning `- cost`.
        None => true,
        Some(token) => match token.kind {
            Kind::Word | Kind::String => false,
            Kind::Comment => true,
            Kind::Operator => !matches!(token.text, ")" | "]"),
        },
    }
}

/// Whether two texts tokenize identically.
///
/// The formatter's proof of safety: identical token sequences mean only whitespace moved. Comment and
/// string *text* is compared too, so a formatter that reindented the inside of a block comment would
/// be caught. `spaced_before` is excluded, since changing spacing is the formatter's whole job.
pub fn equivalent(before: &str, after: &str) -> bool {
    Tokens::new(before)
        .map(|token| (token.kind, token.text))
        .eq(Tokens::new(after).map(|token| (token.kind, token.text)))
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Whether a word is a keyword that takes a space before a following `(`.
///
/// Only the keywords that can precede a parenthesis are listed, since that is the only question
/// asked - a keyword absent here is spaced like a function name, which is the right default for the
/// many Hexaly keywords that are never followed by one.
/// Whether a word is a keyword that takes a space before a following `(`.
///
/// Measured, not assumed. The corpus writes `if (` 346 times against 16, `while (` 14 against 5,
/// `return (` 2 against 0 and `constraint (` 2 against 0 - while `minimize(funcCall)` is written
/// tight, because there `minimize` really is applied to one expression rather than introducing a
/// clause. A keyword absent here is spaced like a function name, which is right for the many Hexaly
/// keywords never followed by a parenthesis.
fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "if" | "else" | "for" | "while" | "do" | "return" | "constraint" | "in" | "with" | "throw" | "try" | "catch"
    )
}

fn word_length(text: &str) -> usize {
    text.find(|character: char| !is_word(character)).unwrap_or(text.len())
}

/// Length of a string literal including both quotes, or to end of line if it is unterminated - which
/// happens constantly in a buffer being typed into.
fn string_length(text: &str) -> usize {
    let mut escaped = false;

    for (offset, character) in text.char_indices().skip(1) {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return offset + 1,
            '\n' => return offset,
            _ => {}
        }
    }

    text.len()
}

/// Length of a line comment, up to but not including the newline.
fn line_comment_length(text: &str) -> usize {
    text.find('\n').unwrap_or(text.len())
}

fn block_comment_length(text: &str) -> usize {
    text.find("*/").map_or(text.len(), |end| end + 2)
}

/// Length of the operator at the start of `text`, preferring the longest match.
fn operator_length(text: &str) -> usize {
    // Ordered longest first. `<-` is the decision operator and the reason this cannot simply take
    // one character at a time; `...` is the range operator, which appears 1038 times in the corpus
    // against 24 for `..`, and splitting it into `..` + `.` changed both spacing and the tokens.
    const OPERATORS: &[&str] = &[
        "<==>", "<->", "...", "<<=", ">>=", "<-", "<=", ">=", "==", "!=", "&&", "||", "=>", "->", "..", "+=", "-=",
        "*=", "/=", "%=", "^=", "|=", "&=", "<<", ">>", "++", "--", "::",
    ];

    OPERATORS
        .iter()
        .find(|operator| text.starts_with(*operator))
        .map_or_else(
            || text.chars().next().map_or(0, char::len_utf8),
            |operator| operator.len(),
        )
}

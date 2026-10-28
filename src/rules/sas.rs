//! Facts about the SAS language that several rules rely on. Keeping one
//! copy means the rules agree on what a statement or a PROC step is.

/// Statement names the lexer emits as plain identifiers rather than `Kw*`
/// tokens.
const STATEMENT_WORDS: &[&str] = &[
    "abort",
    "catname",
    "cards",
    "continue",
    "datalines",
    "dcl",
    "display",
    "dm",
    "endrsubmit",
    "endsas",
    "error",
    "file",
    "go",
    "goptions",
    "goto",
    "infile",
    "leave",
    "lines",
    "link",
    "lock",
    "lostcard",
    "modify",
    "ods",
    "options",
    "otherwise",
    "page",
    "putlog",
    "redirect",
    "remove",
    "replace",
    "resetline",
    "retain",
    "return",
    "rsubmit",
    "sasfile",
    "signoff",
    "signon",
    "skip",
    "sysecho",
    "systask",
    "waitfor",
    "window",
    "x",
];

/// Statements that also take a number suffix: `title2`, `symbol1`, ….
const NUMBERED_STATEMENT_WORDS: &[&str] =
    &["axis", "footnote", "legend", "pattern", "symbol", "title"];

/// A statement name the lexer leaves as a plain identifier.
pub fn is_statement_word(text: &str) -> bool {
    let word = text.to_lowercase();
    if STATEMENT_WORDS.contains(&word.as_str()) {
        return true;
    }
    let base = word.trim_end_matches(|c: char| c.is_ascii_digit());
    NUMBERED_STATEMENT_WORDS.contains(&base)
}

/// PROCs that keep accepting statements after `run;` until `quit;`.
pub const RUN_GROUP_PROCS: &[&str] = &[
    "anova", "arima", "catalog", "datasets", "ds2", "fedsql", "gchart", "glm", "gmap", "gplot",
    "iml", "optmodel", "plm", "plot", "reg", "sql",
];

/// Every statement PROC FORMAT accepts after the PROC statement itself.
pub const FORMAT_STATEMENTS: &[&str] = &["value", "invalue", "picture", "select", "exclude"];

/// The PROC FORMAT statements that define a format.
pub const FORMAT_DEFINITIONS: &[&str] = &["value", "invalue", "picture"];

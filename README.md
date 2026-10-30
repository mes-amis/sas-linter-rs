# sas-linter (Rust)

A configurable lint engine for SAS source files. Single-binary Rust port of the [Ruby `sas-linter` gem](https://github.com/mes-amis/sas-linter), built on Misha Perlov's [`sas-lexer`](https://github.com/mishamsk/sas-lexer) crate. Ships nineteen pluggable rules covering structural defects, cosmetic issues, and source-header conventions, plus a `--format` mode that handles keyword casing, operator spacing, indentation, banner-comment alignment, and assignment alignment.

No Ruby, no RubyGems, no Cargo at runtime — download a prebuilt binary for your platform and run it.

## Installation

### Prebuilt binaries

Every `v*` tag publishes a [GitHub release](https://github.com/mes-amis/sas-linter-rs/releases) with statically-linked binaries. The commands below install the current release, **v0.7.0**, and are kept in step with `Cargo.toml` by `tests/release_versions.rs` — bump the version and the test tells you to update them.

**macOS (Apple Silicon)**

```sh
curl -fsSL -o sas-lint https://github.com/mes-amis/sas-linter-rs/releases/download/v0.7.0/sas-lint-v0.7.0-aarch64-apple-darwin
chmod +x sas-lint && sudo mv sas-lint /usr/local/bin/sas-lint
sas-lint --version
```

**macOS (Intel)**

```sh
curl -fsSL -o sas-lint https://github.com/mes-amis/sas-linter-rs/releases/download/v0.7.0/sas-lint-v0.7.0-x86_64-apple-darwin
chmod +x sas-lint && sudo mv sas-lint /usr/local/bin/sas-lint
sas-lint --version
```

**Linux (x86_64)**

```sh
curl -fsSL -o sas-lint https://github.com/mes-amis/sas-linter-rs/releases/download/v0.7.0/sas-lint-v0.7.0-x86_64-unknown-linux-musl
chmod +x sas-lint && sudo mv sas-lint /usr/local/bin/sas-lint
sas-lint --version
```

**Linux (arm64)**

```sh
curl -fsSL -o sas-lint https://github.com/mes-amis/sas-linter-rs/releases/download/v0.7.0/sas-lint-v0.7.0-aarch64-unknown-linux-musl
chmod +x sas-lint && sudo mv sas-lint /usr/local/bin/sas-lint
sas-lint --version
```

**Windows (x86_64, PowerShell)**

```powershell
$dir = "$env:LOCALAPPDATA\sas-lint"
New-Item -ItemType Directory -Force $dir | Out-Null
Invoke-WebRequest https://github.com/mes-amis/sas-linter-rs/releases/download/v0.7.0/sas-lint-v0.7.0-x86_64-pc-windows-msvc.exe -OutFile "$dir\sas-lint.exe"
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$dir*") { [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User') }
& "$dir\sas-lint.exe" --version   # open a new terminal for plain `sas-lint` to resolve
```

Each asset ships with a `.sha256` next to it on the release page. The binaries have no runtime dependencies (`musl` on Linux), so drop-in anywhere on `PATH` works too.

### From source

```sh
git clone https://github.com/mes-amis/sas-linter-rs
cd sas-linter-rs
cargo install --path .          # installs `sas-lint` into ~/.cargo/bin
# or just build locally:
cargo build --release
./target/release/sas-lint --list-rules
```

Rust 1.82+ required.

### VSCode extension

A first-party extension lives at [`editors/vscode`](editors/vscode). Each `vscode-v*` tag publishes a `.vsix` on the corresponding [GitHub release](https://github.com/mes-amis/sas-linter-rs/releases). The asset name uses the extension's `package.json` version (no `v` prefix), while the release tag has one.

The current extension release is **vscode-v0.4.3** (also kept in step by `tests/release_versions.rs`).

**macOS / Linux**

```sh
curl -fsSL -o /tmp/sas-linter-vscode.vsix https://github.com/mes-amis/sas-linter-rs/releases/download/vscode-v0.4.3/sas-linter-vscode-0.4.3.vsix
code --install-extension /tmp/sas-linter-vscode.vsix
```

**Windows (PowerShell)**

```powershell
Invoke-WebRequest https://github.com/mes-amis/sas-linter-rs/releases/download/vscode-v0.4.3/sas-linter-vscode-0.4.3.vsix -OutFile "$env:TEMP\sas-linter-vscode.vsix"
code --install-extension "$env:TEMP\sas-linter-vscode.vsix"
```

With the [GitHub CLI](https://cli.github.com/), pick up the latest extension release automatically:

```sh
TAG=$(gh api repos/mes-amis/sas-linter-rs/releases \
  --jq '[.[].tag_name | select(startswith("vscode-v"))][0]')
curl -fsSL -o /tmp/sas-linter-vscode.vsix \
  "https://github.com/mes-amis/sas-linter-rs/releases/download/${TAG}/sas-linter-vscode-${TAG#vscode-v}.vsix"
code --install-extension /tmp/sas-linter-vscode.vsix
```

Or grab the `.vsix` from the [release page](https://github.com/mes-amis/sas-linter-rs/releases) (filter by `vscode-v*` tags) and drag it into the VSCode Extensions view → `…` → "Install from VSIX…".

On first activation the extension auto-downloads the matching `sas-lint` binary from the GitHub release for your platform and caches it under VSCode's global storage. Pin a different release with the `sasLinter.version` setting (downloaded on the fly — no extension update needed), or override with the `sasLinter.path` setting to point at a local build instead — useful while iterating on rules.

Features:

- diagnostics on save (`onSave` / `onType` / `manual`, configurable)
- `Format Document` runs `sas-lint --format`
- per-rule Quick Fix code actions for autofix-capable rules
- command palette: `SAS Linter: Lint current file`, `… Run all autofixes on current file`, `… Download / update sas-lint binary`

See [`editors/vscode/README.md`](editors/vscode/README.md) for settings reference and development setup.

## CLI usage

```sh
# Run every rule on a single file
sas-lint path/to/source.sas

# List all registered rules with their description and autofix capability
sas-lint --list-rules

# Run only specific rules
sas-lint --rules malformed_if_condition,identical_if_else_branches src/*.sas

# Use a YAML config (default: config/lint.yaml)
sas-lint --config my-lint.yaml src/*.sas

# Lint without applying any autofixes the config requested
sas-lint --no-autofix src/*.sas

# Reformat files in place: keyword casing + operator spacing + indentation,
# plus any autofix-enabled rules
sas-lint --format --config my-lint.yaml src/*.sas

# --format also realigns banner comment boxes (`**  ...  **;` preambles)
# and runs of `var = expr; **note;` assignments by default — no config
# needed, in the spirit of `terraform fmt`
sas-lint --format legacy/*.sas

# Synthesize input testdata: one <stem>.csv per source in the output dir
sas-lint synthesize sources/* -o testdata/inputs
sas-lint synthesize --size 1000 --seed 7 algo.sas -o algo_inputs.csv

# Reshape the strategy mix: only single- and combo-predicate rows, 3:1
sas-lint synthesize --weights single=3,combo=1 --combo-size 2-4 algo.sas -o algo.csv

# Fewest rows that drive every `if` both ways (100% branch coverage)
sas-lint synthesize --minimal algo.sas -o algo_min.csv
```

Exit codes: `0` clean, `1` findings, `2` invalid args.

### `synthesize`

Walks each source's logical graph (array declarations, `VAR = VAR;`
self-declarations, `if`-head predicates including nested `if … then do`
guards) to find the input variables, builds a value set per input, and
writes `--size` sampled rows (default 500) as CSV. A variable read on the
right of an assignment and never assigned (`ht = raw_ht / 100;`) is an
input too. Inputs whose values are all string literals (or `$` arrays), and
inputs the catalog documents as `yyyymmdd` dates, default to `""`;
everything else to `0`.

Conditions are modelled as comparisons against literals in either order
(`x > 3`, `0 < x`), chained ranges (`5 le x le 70`), `in` lists, `sum()`
thresholds, `max()` comparisons (`max(a, b) ge 2` holds when any argument
does, as missing sorts below every number), comparisons between two variables (`dob < ref_date`) and
`missing(x)`; a lone `.` is the missing value. A parenthesised comparison
compared with 1 or 0 (`(x = 0) = 0`) is the comparison or its negation. A condition on a derived
variable is rewritten into a condition on what decides it, when every
assignment to the variable is a constant (`flag = 1;`, `flag = .;`), a copy
(`xv = v;`) or a unit conversion (`m = (inches * 0.0254);`): `flag in (0,1)`
becomes the conditions under which one of the assignments giving 0 or 1
runs and no later assignment overrides it. A sum or difference of
variables and constants (`n = 7 - (a + b);`, or of comparisons,
`n = (a = 0) + (b = 0);`) is rewritten over the values each variable's
guard or catalog spec lists, or that a derived variable can be given
(`x = iD1 - 1;` then `n = x + y;`), and is missing when any variable is;
`n = sum(a, b, max(of c d));` skips missing arguments and is missing only
when all are. A sum with few combinations of values becomes one
alternative per combination the comparison accepts; a larger one becomes
a count. `a - b ge 1` compares the difference of two variables, and
`x > = 0` is read as `x >= 0`, as SAS does. `x not in (…)` and
`x not eq k` are the negated comparisons. A counter (`n = 0;` then
`if c then n = n + 1;` or the sum statement `if c then n + 1;`, or
`n = n + x - 1` by a variable whose guard lists its values) is rewritten
as a count of the steps whose conditions hold, so `n ge 2` becomes "at
least two of those conditions". An `end;` left unmatched by a validity
guard commented out above it doesn't end the program. A declared input
(`x = x;`) that the source also assigns keeps its input value unless one
of those assignments runs.

Rows come from a weighted mix of strategies:

| Strategy       | Default weight | Row                                                        |
| -------------- | -------------- | ---------------------------------------------------------- |
| `baseline`     | 0.15           | every input at its default                                 |
| `single`       | 0.45           | satisfy one predicate                                      |
| `combo`        | 0.25           | satisfy `--combo-size` predicates at once (default `2-6`)  |
| `perturbation` | 0.10           | baseline, each cell redrawn at `--perturbation-prob` (0.05) |
| `chaos`        | 0.05           | every cell sampled uniformly from its value set            |

`--weights` replaces the mix, as `name=weight,…`. The weights are relative,
so they don't need to sum to 1, and any strategy you leave out gets 0.
`--weights perturbation=1 --perturbation-prob 0.2` produces only
perturbation rows, each with about a fifth of its cells flipped.
`--combo-size` takes `MIN-MAX`, or a bare `N` for exactly N predicates.
Output is deterministic for a given `--seed` (default 42).

`--minimal` skips the weighted mix. It writes the smallest row set it can
find that sends every `if` down both its true and its false branch,
covering 100% of the branches in the modelled graph. Rows are sorted by
the first branch each one reaches in source order. Reachability follows
`then do` nesting, `else` / `else if` chains, and subsetting `if X;`
statements. A condition inside an array `do` loop counts as covered when
any iteration takes the branch. The set comes from a greedy set cover and
is often, though not provably, the true minimum. Conditions the parser
can't model (function calls, variables computed by other arithmetic) are
assumed to hold. A range bounded on both sides (`lo < x < hi`) is met with
its midpoint rather than an edge, so rows stay sensible for arithmetic
further on that the graph doesn't model (`bmi = wt / (ht * ht)`).

`--minimal` also puts every input the catalog documents at its
boundaries, which takes a few extra rows: the minimum and maximum
documented values, and one step outside each end. For a set like
`0,1,2,3,8`, the ends are its smallest and largest values. A step is 1
for integer specs. For decimal ranges it's the finest decimal place of
the bounds, and at least 0.1. `0-3` therefore yields -1, 0, 3 and 4, and
`0.5-40.0` yields 0.4, 0.5, 40.0 and 40.1. Boundary values are packed
into rows that already exist wherever they don't change those rows'
branch outcomes and the cell wasn't deliberately put mid-range.

`--minimal` also aims for every constant a derived variable can be
assigned (`score = 0` … `score = 6`, `flag = 2`), each in some row, so
every outcome of the algorithm is exercised, not just every branch.
Branch coverage alone misses combinations: a scale's `0` needs every
increment off at once.

Stdout reports `covers N/M branch outcomes`, plus `N/M assigned values`
when the source assigns any constants the graph can model and the number
of boundary values placed when the catalog documents any input. Stderr
names each `file:line` outcome and each `variable = value` that no row
reaches, either because it's unreachable or because it's beyond what the
sampler can pin. `--minimal`
can't be combined with `--size`, `--weights`, `--perturbation-prob` or
`--combo-size`.

Value sets are also seeded from the acceptable-values catalog configured
for `variable_value_out_of_known_range` in `--config` (`csv_paths`,
`name_column`, `values_column`, `delimiter`, `name_match`), so no extra
setup is needed when that rule is already configured.

`-o` is a directory unless it ends in `.csv`, which writes one file and
needs exactly one source. A `.sas` / `.txt` extension is dropped from the
output name.

## YAML config

Every rule with options, plus its defaults. Rules omitted from the config default to enabled with no options, so adding a new rule to `sas-linter` won't silently disable it for users with existing configs. To suppress a rule, list it with `enabled: false`.

```yaml
rules:
  # ── Structural / semantic rules ─────────────────────────────────────
  unreachable_inner_branch_value:
    enabled: true              # default for every rule

  unreachable_else_if_branch:
    enabled: true

  identical_if_else_branches:
    enabled: true

  malformed_if_condition:
    enabled: true

  commented_out_guard:
    enabled: true

  choose_one_template:
    enabled: true

  missing_assignment_semicolon:
    enabled: true
    autofix: false             # rule supports autofix; off by default

  proc_statement_missing_semicolon:
    enabled: true
    autofix: false             # insert the `;` that closes the proc statement

  invalid_assignment_target:
    enabled: true
    autofix: false             # join the space-separated words with `_`

  malformed_label_statement:
    enabled: true
    autofix: false             # insert the missing `=` / terminating `;`

  unbalanced_do_block:
    enabled: true              # report-only; skipped on files with %do or proc sql select

  unterminated_comment:
    enabled: true
    autofix: false             # end the comment on its first line when the next line is the swallowed code
                               # (replaces star_comment_swallows_code; that id is still accepted as an alias)

  inconsistent_variable_case:
    enabled: true
    autofix: false             # rewrite every minority casing to the most-common form

  format_for_unknown_variable:
    enabled: true              # skipped automatically when the file uses set/merge/update/infile/input

  unescaped_quote_in_string:
    enabled: true
    autofix: false             # double the quote (`''`) so it stays in the string

  orphan_format_range:
    enabled: true
    autofix: false             # remove the stray `;` that closed the value statement early

  statement_not_valid:
    enabled: true              # data steps and open code only; PROC bodies are skipped

  undefined_format:
    enabled: true              # skipped when the file has no proc format, or uses fmtsearch / %include

  in_range_on_fractional_value:
    enabled: true
    autofix: false             # rewrite `VAR in (N:M)` to `N le VAR le M`
    half_open_ranges: false    # also treat `18 le VAR lt 30` as a sign VAR holds fractions
    csv_paths: []              # optional catalog: a decimal range (`0.0-99.9`) marks VAR as fractional
    name_column: "Variable"    # same catalog options as variable_value_out_of_known_range
    values_column: "Acceptable Values"
    delimiter: ","
    name_match: case_insensitive

  catalog_vector_mismatch:
    enabled: true              # checks the catalog itself, once per run, not each source
    # csv_paths: [...]         # omitted → the variable_value_out_of_known_range catalog and columns
    vector_column: "Vector"    # default; the values column spelled out one value at a time

  invalid_numeric_literal:
    enabled: true              # reject INTEGER_LITERAL tokens the lexer accepts but SAS doesn't (`1f`, `1d2`, …)

  variable_value_out_of_known_range:
    enabled: true
    csv_paths:                          # empty list = rule is a no-op
      - metadata/variables.csv
      - metadata/variables-extra.csv
    name_column: "Variable"             # default
    values_column: "Acceptable Values"  # default
    name_match: case_insensitive        # case_insensitive | exact
    delimiter: ","                      # CSV column separator: "," | ";" | "\t"

  invalid_conventional_variable:
    enabled: true
    pattern: '^[A-Z]+_\d+$'             # required regex; rule is a no-op when omitted
    csv_paths:                          # catalog files; empty list = rule is a no-op
      - metadata/known-variables.csv
    name_column: "Name"                 # default
    name_match: case_insensitive        # case_insensitive | exact
    delimiter: ","                      # CSV column separator: "," | ";" | "\t"
    allow_list:                         # strings = literal match, /…/ = regex
      - ABC_99
      - '/^ABC_\d+_[a-z]$/'

  # ── Source-hygiene rules (all support autofix) ──────────────────────
  trailing_whitespace:
    enabled: true
    autofix: false

  tab_expansion:
    enabled: true
    autofix: false
    width: 8                   # tab stop width
    hook: before_format        # default for tab_expansion / line_endings / encoding_issues

  source_headers:
    enabled: true
    autofix: false             # re-wrap leaked header text as 90-char **…**; rows

  line_endings:
    enabled: true
    autofix: false             # collapse \r\r\n → \r\n; lone \r → dominant ending

  encoding_issues:
    enabled: true
    autofix: false
    use_defaults: false        # apply built-in smart-quote / dash / non-breaking-space map
    replacements:              # project-specific byte→ASCII rewrites (run BEFORE defaults)
      "—": "--"

# ── Formatter (used by `--format` mode) ───────────────────────────────
format:
  keywords: preserve           # preserve | upper | lower
  operator_spacing: true       # normalize spaces around binary operators and after commas
  indent_width: 2              # indentation width; 0 or omit to disable
  align_comment_banners: true  # realign `**  ...  **;` banner boxes: columns split on 2+ spaces,
                               # uniform `**;` terminator width, borders/dividers stretched to match,
                               # off-by-one row indents snapped, stacked `**..**; **..**;` lines split
  align_assignments: true      # align `=` and trailing comments across runs of simple assignments
```

`enabled` and `autofix` are accepted on every rule. Options not listed above are ignored.

`hook` (`before_format` | `after_format`) is also accepted on every rule and controls when its autofix runs in `--format` mode. `tab_expansion`, `line_endings` and `encoding_issues` default to `before_format`, so the formatter's alignment and indentation see the normalized bytes and a single save settles; every other rule defaults to `after_format`. Plain lint runs (no `--format`) ignore it.

## Library usage

Add to `Cargo.toml`:

```toml
[dependencies]
sas-linter = { git = "https://github.com/mes-amis/sas-linter-rs" }
```

Then:

```rust
use sas_linter::{Config, Linter};

// All registered rules with default options
let linter = Linter::with_default_rules();

// Subset by rule id (overrides config; no per-rule options)
let linter = Linter::from_ids(&["malformed_if_condition".into()])?;

// From a parsed YAML config
let cfg = Config::load(std::path::Path::new("lint.yaml"))?;
let linter = Linter::from_config(&cfg)?;

// Lint a source string in memory
let findings = linter.lint(source, "demo.sas");
for f in &findings {
    println!("{}", f);   // path:line:col: [rule_id] message
}

// Lint a file. If any rule has autofix enabled and changed the source,
// the file is rewritten in place.
let findings = linter.lint_file(std::path::Path::new("path/to/source.sas"))?;
```

## Built-in rules

| rule id | description |
|---|---|
| `unreachable_inner_branch_value` | Outer `if VAR in (S) then do;` guards an inner branch that tests `VAR` against a value outside `S`. A colon range in an `in` list (`VAR in (1:4)`) stands for its whole-number members, up to 1000 of them; a range's dead members are reported as one finding (`values 5:10`). Every `VAR in (…)` / `VAR = N` conjunct of an `and` chain is checked, in nested `if`s and `else if` arms alike, and a compound guard (`if A in (…) and B in (…) then do;`) restricts each of its variables. Conditions with `or` (`|`, `!`), `not` or `ne` are left alone; `&` is `and`. String values compare as SAS compares them (quote style and trailing blanks ignored). A guard stops restricting a variable once the block may assign it (assignment, sum statement, `do` index, `input`, `call` argument, array element), and stops restricting every variable at `set` / `merge` / `update` / `modify`. |
| `unreachable_else_if_branch` | An `else if` arm whose condition is already covered by an earlier arm of the same `if` / `else if` chain, so it can never fire. Catches exact repeats (conjunct order, `in (…)` order, case and whitespace ignored) and subsumption — a smaller `in` set (colon ranges such as `in (1:5)` count as their whole-number members), a narrower `lt`/`le`/`gt`/`ge` range, or a point value inside an earlier range, on the same variable. Arms with function calls, `or`, negation or arithmetic are only matched by an exact repeat; each nested chain is checked on its own. Report-only. |
| `identical_if_else_branches` | `if COND then S; else S;` with identical bodies — almost always a copy-paste error. Names and keywords compare case-insensitively; string literals compare exactly. |
| `commented_out_guard` | SAS line-comment `* if ... then do;` pattern indicating a disabled outer validity guard. |
| `choose_one_template` | `** CHOOSE ONE OF THE BELOW STATEMENTS;` banner indicating a broken-by-default source. |
| `trailing_whitespace` | Trailing spaces/tabs at end of line, outside multi-line string literals and datalines (whose blanks are data). |
| `tab_expansion` | Tab characters that should be spaces (configurable width). Tabs inside string literals (a literal tab in `dlm="…"`) and datalines are data and are left alone. |
| `source_headers` | Header text that leaks out of its `**` comment and runs as code — after a `;` inside the text (`** CHECKED BY: A; B **;` ends at the first `;`), or a continuation line missing its `**` — and header boxes whose missing `;` lets the comment swallow the code after it. Code that merely follows a comment on the same line is left alone. Autofix re-wraps the line as a 90-char `**  …  **;` row, turning `;` inside the text into `,`. |
| `line_endings` | Broken line terminators: a doubled CR (`\r\r\n`) or a lone CR. Autofix collapses `\r\r\n` to `\r\n` and turns a lone `\r` into the file's dominant ending. |
| `encoding_issues` | Smart-quote / em-dash / Win-1252 punctuation that confuses downstream tooling. Autofix keeps strings and `*` comments valid: a replacement that yields the enclosing string's quote is doubled (`'It''s'`). Possible Mac Roman mojibake (Ð Ò Ó Ô Õ, which are also real letters) is reported but never rewritten. |
| `malformed_if_condition` | Empty conditions, missing operators, orphan `then`, unbalanced parens, etc. Understands PROC SQL `case when … then`, prefix comparisons (`=:`, `in:`) and special missing values (`.a`–`.z`, `._`). |
| `missing_assignment_semicolon` | Assignment statements followed by an inline `**` or `*` comment but no terminating `;`, so the comment is parsed as part of the expression. Prose is recognised by two bare words side by side, which no expression contains (`x = x ** Some description;`); a single word after the marker stays ambiguous with a real operand (`radius ** two`) and is left alone. Autofix inserts the `;` before the marker. |
| `proc_statement_missing_semicolon` | A `proc` statement with no `;` absorbs the statement after it as PROC options (`proc format value …` → `ERROR 22-322`). Checks `proc format` (`value` / `invalue` / `picture`), `sort` (`by`), `freq` (`tables`), `means` / `summary` (`var` / `class`) and `print` (`var`). Autofix inserts the `;`. |
| `invalid_assignment_target` | Data-step assignment whose target is several space-separated words (`Predicted risk = ...`) — SAS variable names cannot contain spaces. Suggests the underscore-joined name; autofix applies it. |
| `unterminated_comment` | A `*` or `**` comment — full-line or trailing — with no `;` on its line (often a `:` or `.` typo, or a `** … **` box line missing its `;`). SAS runs the comment to the next `;`, silently swallowing the code in between. Fires only when a swallowed line is a SAS statement, so comments that merely wrap prose, header boxes and comments that run to end of file stay quiet. Severity error. Autofix ends the comment on its first line when the swallowed code is the very next line. Replaces `star_comment_swallows_code`, which is still accepted in configs and `--rules` as an alias. |
| `unbalanced_do_block` | `do` / `end` nesting that doesn't balance — each unclosed `do` is reported at its own line, plus any stray `end`. In SAS this is a hard compile error (`ERROR 117-185: There were N unclosed DO blocks.`) reported with no line number. Data-step `select` blocks, PROC SQL `case … end`, PROC TEMPLATE `define` / `edit` and PROC DS2 `method` blocks consume an `end` too and are tracked; other PROCs aren't counted. A file goes silent only when a `%do` / `%macro` body leaves a block open or closes one it didn't open. Report-only by design — where a missing `end` belongs is a semantic decision. |
| `malformed_label_statement` | `label VAR 'text';` is missing the `=` between the variable name and the label string, or a `label` statement is missing its `;` so a following statement keyword (`attrib`, `label`, `format`, `if`, `run`, …) is swallowed into it. Autofix inserts the `=` / `;`. |
| `invalid_numeric_literal` | INTEGER_LITERAL tokens whose text isn't actually a valid SAS numeric literal (e.g. `1f`, `1d2`). |
| `variable_value_out_of_known_range` | `VAR = N` / `VAR in (...)` tests that stand alone as an `and` / `or` operand of an `if` condition (not inside arithmetic such as `y + VAR = 5`) use literals outside the variable's documented acceptable values. A colon range (`VAR in (0:9)`) is checked member by member and reported once at the range (`values 6:9`). Loads the catalog from one or more CSVs with configurable column names and column separator (`,`, `;`, tab). |
| `invalid_conventional_variable` | Identifier matches a configured naming convention regex but isn't in the catalog. Reports the closest Levenshtein matches as "did you mean?" hints. `allow_list` entries are matched literally; entries wrapped in `/.../` are matched as regexes. No-op until both `pattern` and `csv_paths` are configured. |
| `inconsistent_variable_case` | Identifier appears with more than one casing in the same file (`myVar` vs `MyVar`). SAS treats both as the same variable; autofix rewrites every minority spelling to the most-common form. Only data-step variable positions count — dataset names, function / CALL names, statement labels, `format.` / `lib.member` references and PROC step bodies are skipped. |
| `format_for_unknown_variable` | `format` / `informat` / `attrib` / `label` statement assigns a format or label to a variable that's referenced nowhere else in the file — almost always a typo. PROC steps are skipped (their statements name the input dataset's variables), as are files that pull in columns via `set` / `merge` / `update` / `modify` / `infile` / `input` (including `if … then set …`). Variable ranges (`x1-x3`, `a--c`) are understood. |
| `unescaped_quote_in_string` | A quote inside a string that matches its delimiter (`'Children's …'`) ends the string early: SAS reads the rest as code, and the next quote opens a new string that can swallow `;`, `end;` and `run;`. Recognised by a word glued to the closing quote — typed literals (`'…'d`, `'…'n`, `'…'x`) and keywords (`'a'then`) are fine. Severity error. Autofix doubles the quote (`''`). |
| `orphan_format_range` | Inside `proc format`, a statement that starts with a literal, range, `low` / `high` or `other` followed by `=`, right after a `value` / `invalue` / `picture` statement — a stray `;` closed the definition early and SAS fails with `ERROR 180-322`. Autofix removes that `;`. |
| `statement_not_valid` | A statement in open code or a data step that doesn't start like any SAS statement — bare text (a filename line above the header), stray punctuation (`*/.`), a `*` comment cut short by an embedded `;`. Accepts keywords, assignments (`x =`, `a{i} =`, `'n'n =`), sum statements, labels, hash method calls and macro code. PROC bodies are skipped; `run;` ends a PROC except run-group PROCs (`datasets`, `reg`, `sql`, …), which end at `quit;`. SAS fails with `ERROR 180-322`. Report-only. |
| `undefined_format` | `format` / `informat` / `attrib … format=` names a format that the file's `proc format` doesn't define (`value` / `invalue` / `picture`) and isn't a SAS-supplied format — SAS fails with `ERROR 48-59`. Case-insensitive; a leading `$` is part of the name and width digits are ignored. Skipped on files with no `proc format` or that use `fmtsearch` / `%include`, whose formats may come from elsewhere. |
| `in_range_on_fractional_value` | `VAR in (… N:M …)` where `VAR` can hold a fraction. A colon range in an `in` list stands for the whole numbers `N, N+1, …, M`, so `bmi in (5:70)` is false for `28.4` — SAS compiles it without a note. `VAR` counts as fractional when the file assigns it a division, a non-integer literal, a negative or fractional power, or a function such as `sqrt` / `mean` / `exp` (not inside `floor`, `int`, `round(x)` and other whole-number functions), or else compares it with a non-integer literal (`bmi < 18.5`), or — with `half_open_ranges: true` — as a half-open range (`18 le bmi lt 30`, off by default since whole-number values are written that way too). Failing those, a configured catalog (`csv_paths`, read like `variable_value_out_of_known_range`'s) whose entry for `VAR` is a decimal range (`0.0-99.9`) counts. Variables it can't reason about are left alone. One finding per range. Autofix rewrites a list that is a single range to `N le VAR le M`; mixed lists and `not in` are report-only. |
| `catalog_vector_mismatch` | A catalog row whose values column and vector column name different values: `1-3,8` beside `1,2,3,4,5,6,7,8` (a vector filled in from min to max admits the gap `4-7`), only one of the two a value list (a description with an unquoted delimiter shifted the columns), or one of them empty. Both are read as whole numbers and `lo-hi` ranges and compared as sets, so `0-2` and `0,1,2` agree; rows where neither is a value list are skipped, and a range of 1000+ values agrees with a vector of its two ends. Reads the `variable_value_out_of_known_range` catalog and columns unless given its own `csv_paths`, so it runs as soon as that catalog is configured; `vector_column` (default `Vector`) names the column, and a catalog without one is skipped. Reported once per run at `catalog.csv:ROW:1`, however many sources are linted. |

`sas-lint --list-rules` prints the same set with autofix capability.

## Writing a custom rule

Implement the `Rule` trait. The minimum surface is `id`, `description`, and `check`; override `supports_autofix` + `autofix` to support rewrite.

```rust
use sas_linter::finding::{Finding, Severity};
use sas_linter::rule::{CheckContext, Rule};

pub struct ForbidFoo;

impl Rule for ForbidFoo {
    fn id(&self) -> &'static str { "forbid_foo" }
    fn description(&self) -> &'static str { "Flag occurrences of FOO." }
    fn severity(&self) -> Severity { Severity::Warning }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        ctx.tokens.default.iter()
            .filter(|t| t.text == "FOO")
            .map(|t| Finding {
                path: ctx.path.to_string(),
                line: t.start_line,
                column: t.start_column + 1,
                rule: "forbid_foo",
                message: "FOO is forbidden".into(),
                severity: Severity::Warning,
            })
            .collect()
    }
}
```

To register the rule alongside the built-in set, add it to `src/rules/mod.rs::all_metas()` together with a `meta()` factory that exposes the rule's id, description, autofix capability, and config-driven constructor. The Rust port uses an explicit list rather than the Ruby gem's auto-registry — the trade-off is one more line per rule for the ability to compile-check the registry.

Or — if you're consuming `sas-linter` as a library — instantiate your `Box<dyn Rule>` directly and push it onto a custom `Linter { rules: vec![...] }`.

## Testing

```sh
cargo test            # unit + integration tests
cargo clippy          # lints
cargo fmt -- --check  # formatting
```

Fixtures live under `tests/fixtures/lints/<rule_id>/` as a `lint.sas` (demonstrates the bug, expected to fire) and `clean.sas` (same shape, fixed, expected to be silent) pair. The parametric suite in `tests/fixtures_smoke.rs` walks them all.

## License

[GNU Affero General Public License v3.0 or later](LICENSE) — chosen to match the upstream `sas-lexer` crate (which `sas-linter` links against). © Mon Ami, Inc.

Practical implications:

- **Internal / personal use** has no obligations beyond preserving notices.
- **Redistribution** (shipping the binary inside a container image or product) requires offering the complete corresponding source under AGPL-3.0.
- **Network use** (running `sas-linter` as a backend that users interact with remotely) triggers the AGPL's source-disclosure clause for those network users.
- **Combined works** with `sas-linter` must be licensed under AGPL-compatible terms.

If those terms don't fit your use case, run a standalone lint job (CLI / CI step) against the prebuilt binary instead of embedding the library.

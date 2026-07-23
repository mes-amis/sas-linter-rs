# sas-linter (Rust)

A configurable lint engine for SAS source files. Single-binary Rust port of the [Ruby `sas-linter` gem](https://github.com/mes-amis/sas-linter), built on Misha Perlov's [`sas-lexer`](https://github.com/mishamsk/sas-lexer) crate. Ships nineteen pluggable rules covering structural defects, cosmetic issues, and source-header conventions, plus a `--format` mode that handles keyword casing, operator spacing, indentation, banner-comment alignment, and assignment alignment.

No Ruby, no RubyGems, no Cargo at runtime — download a prebuilt binary for your platform and run it.

## Installation

### Prebuilt binaries

Every `v*` tag publishes a [GitHub release](https://github.com/mes-amis/sas-linter-rs/releases) with statically-linked binaries for:

| target                          | runs on             |
|---------------------------------|---------------------|
| `aarch64-apple-darwin`          | macOS (Apple Silicon) |
| `x86_64-apple-darwin`           | macOS (Intel)       |
| `x86_64-unknown-linux-musl`     | Linux x86_64        |
| `aarch64-unknown-linux-musl`    | Linux arm64         |
| `x86_64-pc-windows-msvc`        | Windows x86_64      |

```sh
# Pick the artifact for your platform, drop it on $PATH:
curl -fsSL -o sas-lint \
  https://github.com/mes-amis/sas-linter-rs/releases/latest/download/sas-lint-<TAG>-<TARGET>
chmod +x sas-lint
./sas-lint --list-rules
```

On Windows, grab `sas-lint-<TAG>-x86_64-pc-windows-msvc.exe` from the release page and put it somewhere on `%PATH%`.

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

```sh
# Install from the command line (substitute the current vscode-v* tag and matching version):
TAG=vscode-v0.3.0
VER=0.3.0
curl -fsSL -o /tmp/sas-linter-vscode.vsix \
  "https://github.com/mes-amis/sas-linter-rs/releases/download/${TAG}/sas-linter-vscode-${VER}.vsix"
code --install-extension /tmp/sas-linter-vscode.vsix
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
```

Exit codes: `0` clean, `1` findings, `2` invalid args.

## YAML config

Every rule with options, plus its defaults. Rules omitted from the config default to enabled with no options, so adding a new rule to `sas-linter` won't silently disable it for users with existing configs. To suppress a rule, list it with `enabled: false`.

```yaml
rules:
  # ── Structural / semantic rules ─────────────────────────────────────
  unreachable_inner_branch_value:
    enabled: true              # default for every rule

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

  invalid_assignment_target:
    enabled: true
    autofix: false             # join the space-separated words with `_`

  unterminated_comment:
    enabled: true
    autofix: false             # append `;` to `**…**` lines whose missing terminator eats the next line

  inconsistent_variable_case:
    enabled: true
    autofix: false             # rewrite every minority casing to the most-common form

  format_for_unknown_variable:
    enabled: true              # skipped automatically when the file uses set/merge/update/infile/input

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

  source_headers:
    enabled: true
    autofix: false             # rewrap **…**; 90-char header rows when true

  line_endings:
    enabled: true
    autofix: false             # collapse \r\r\n → \r\n; lone \r → dominant ending

  encoding_issues:
    enabled: true
    autofix: false
    use_defaults: false        # apply built-in smart-quote / em-dash / Win-1252 map
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
| `unreachable_inner_branch_value` | Outer `if VAR in (S) then do;` guards an inner branch whose comparison values aren't all in `S`. |
| `identical_if_else_branches` | `if COND then S; else S;` with identical bodies — almost always a copy-paste error. |
| `commented_out_guard` | SAS line-comment `* if ... then do;` pattern indicating a disabled outer validity guard. |
| `choose_one_template` | `** CHOOSE ONE OF THE BELOW STATEMENTS;` banner indicating a broken-by-default source. |
| `trailing_whitespace` | Trailing spaces/tabs at end of line. |
| `tab_expansion` | Tab characters that should be spaces (configurable width). |
| `source_headers` | Restore the `**...**;` 90-char header convention to broken sources. |
| `line_endings` | Mixed or non-CRLF line terminators (configurable target). |
| `encoding_issues` | Smart-quote / em-dash / Win-1252 byte sequences that confuse downstream tooling. |
| `malformed_if_condition` | Empty conditions, missing operators, orphan `then`, unbalanced parens, etc. |
| `missing_assignment_semicolon` | Assignment statements followed by an inline `**` comment but no terminating `;`. |
| `invalid_assignment_target` | Data-step assignment whose target is several space-separated words (`Predicted risk = ...`) — SAS variable names cannot contain spaces. Suggests the underscore-joined name; autofix applies it. |
| `unterminated_comment` | A standalone `** … **` comment whose missing `;` lets the SAS lexer extend the comment into the next line of real code, silently swallowing it. Autofix appends the `;`. |
| `star_comment_swallows_code` | A trailing `*` comment on a code line that isn't terminated on its own line (commonly ending in `:` — a typo for `;`). SAS runs the comment to the next `;`, silently disabling the following statement. |
| `malformed_label_statement` | `label VAR 'text';` is missing the `=` between the variable name and the label string. Autofix inserts the `=`. |
| `invalid_numeric_literal` | INTEGER_LITERAL tokens whose text isn't actually a valid SAS numeric literal (e.g. `1f`, `1d2`). |
| `variable_value_out_of_known_range` | `if VAR = N` / `if VAR in (...)` literals fall outside the variable's documented acceptable values. Loads the catalog from one or more CSVs with configurable column names and column separator (`,`, `;`, tab). |
| `invalid_conventional_variable` | Identifier matches a configured naming convention regex but isn't in the catalog. Reports the closest Levenshtein matches as "did you mean?" hints. `allow_list` entries are matched literally; entries wrapped in `/.../` are matched as regexes. No-op until both `pattern` and `csv_paths` are configured. |
| `inconsistent_variable_case` | Identifier appears with more than one casing in the same file (`myVar` vs `MyVar`). SAS treats both as the same variable; autofix rewrites every minority spelling to the most-common form. Skips proc-format definitions and `format.` / `lib.member` references. |
| `format_for_unknown_variable` | `format` / `informat` / `attrib` statement assigns a format to a variable that's referenced nowhere else in the file — almost always a typo. Skipped on files that pull in columns via `set` / `merge` / `update` / `infile` / `input`. |

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

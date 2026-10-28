# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Rust port of the Ruby [`sas-linter`](https://github.com/mes-amis/sas-linter) gem. Single binary `sas-lint` + library `sas_linter`, built on `sas-lexer`. Ships 27 pluggable rules plus a `--format` mode (keyword casing / operator spacing / indent, and on-by-default banner-comment + assignment alignment passes in `src/align.rs`), and a `synthesize` subcommand (`src/synthesize.rs`) that samples CSV input testdata from each source's `if`-predicate graph — a weighted strategy mix (`--weights`), or `--minimal`'s greedy 100%-branch-coverage row set. The acceptable-values catalog loader (`src/value_specs.rs`) is shared between `synthesize` and `variable_value_out_of_known_range`. Targets Rust 1.82+.

## Commands

```sh
cargo build --release           # produces target/release/sas-lint
cargo test                      # unit + integration tests
cargo test <name>               # single test by name substring
cargo test --test fixtures_smoke # run one integration test file
cargo clippy
cargo fmt -- --check
./target/release/sas-lint --list-rules           # all rules, with [autofix] markers
./target/release/sas-lint --rules <id1>,<id2> file.sas
./target/release/sas-lint --config lint.yaml file.sas
./target/release/sas-lint --format --config lint.yaml file.sas
./target/release/sas-lint synthesize sources/* -o out_dir   # one <stem>.csv per source
```

CLI exit codes (mirror the Ruby gem; tests assert against these): `0` clean, `1` findings, `2` misuse / missing file / unknown rule.

## Architecture

Pipeline: **`Config`** (YAML, `src/config.rs`) → **`Linter`** (`src/linter.rs`) holds `Vec<Box<dyn Rule>>` → tokenize source into **`TokenStream`** (`src/token.rs`) → run each rule's `check` → collect `Finding`s → run autofix-enabled rules' `autofix` sequentially over the source string → write file iff bytes changed. `--format` mode runs `Formatter::format` (`src/formatter.rs`) first, then autofix rules on the formatted text.

**`TokenStream` has two channels.** `default` excludes `HIDDEN` + `COMMENT` (what most rules walk); `all` keeps everything — needed by comment-aware rules (`commented_out_guard`, `unterminated_comment`). Token coordinates are 1-based line, **0-based column**; rules add +1 when emitting findings. Tokens also carry byte offsets for autofix passes that splice the source by byte range.

**Source encoding.** SAS files in the wild are commonly Win-1252 / Latin-1, not UTF-8. `Linter::lint_file` and `format_file` go through `encoding::read_source` (`src/encoding.rs`) which transcodes before tokenizing — don't bypass it.

### Adding a rule

1. Create `src/rules/<rule_id>.rs` with a struct implementing `Rule` (`src/rule.rs`) — at minimum `id`, `description`, `check`; override `supports_autofix` + `autofix` for rewriters.
2. Export a `pub fn meta() -> RuleMeta` exposing id, description, autofix capability, `default_factory`, and `config_factory` (the latter receives the rule's `serde_yaml::Value` options map).
3. Register the module in `src/rules/mod.rs` and append `<rule>::meta()` to `all_metas()`. This is an **explicit list, not auto-registry** (deliberate divergence from the Ruby gem — costs one line per rule, buys a compile-time registry check).
4. Add fixtures at `tests/fixtures/lints/<rule_id>/lint.sas` (must fire ≥1 finding) and `clean.sas` (must be silent). Wire the pair into the `PAIRS` table in `tests/fixtures_smoke.rs` — that parametric suite drives both halves. Several fixture dirs can target one rule id (e.g. `unreachable_inner` and `unreachable_inner_eq` both exercise `unreachable_inner_branch_value`).
5. Document it in `README.md`: an entry in the `## YAML config` template and a row in the `## Built-in rules` table (`tests/readme_rules.rs` enforces both). An autofix rule also goes in `AUTOFIX_RULES` in `editors/vscode/src/rules.ts`.

Retiring or merging a rule id: add it to `RETIRED_IDS` in `src/rules/mod.rs` so existing configs and `--rules` lists keep resolving.

### Config semantics (Ruby-parity contracts)

- **Omitted rules stay enabled.** `rules_from_config` (`src/rules/mod.rs`) makes two passes: explicit entries first, then defaults for any rule not mentioned. New rules never silently regress existing user configs.
- **`enabled: false`** is the only way to suppress a rule.
- **Missing config file** is not an error — `Config::load` returns the default (empty) config.
- **`--no-autofix`** calls `Config::strip_autofix` which walks every rule entry and forces `autofix: false`, guaranteeing a dry run.
- **`--rules <ids>` overrides `--config`** entirely — runs the listed ids with default options, no per-rule config.

### Autofix ordering

Inside `lint_with_fixes`: all rules' `check` runs over the original source first (so findings reflect the pre-fix state); then autofix-enabled rules run **sequentially** over an accumulating string, in the order of `AUTOFIX_ORDER` (`src/rules/mod.rs`) — not config order. It is grouped in phases (bytes → comment boundaries → statement boundaries → names) so each fix sees the text earlier phases settled and one save reaches a fixed point; `tests/autofix_order.rs` requires every autofix rule to appear in it exactly once. Place a new rewrite rule in the phase whose output it depends on.

### Report precedence

After every rule's `check` runs, `REPORT_PRECEDENCE` (`src/rules/mod.rs`) drops a general rule's finding when a more specific rule reports the same line (e.g. `source_headers` explains the `statement_not_valid` / `malformed_if_condition` noise from header text running as code). It only applies while the specific rule is enabled, so a defect is never left unreported. Prefer adding a pair here over making one rule call into another.

**`TokenStream.default` is what SAS runs.** There is no comment heuristic beyond the lexer's: text after a `;` inside a `** … **;` line is code, and rules see it.

In `--format` mode (`Linter::format_source`) autofixes are split by `Rule::hook()`: `before_format` rules (default for `tab_expansion`, `line_endings`, `encoding_issues`) run before the formatter so its column math sees normalized bytes; everything else runs after. A per-rule `hook:` config key overrides the default (applied via the `HookOverride` wrapper in `src/rules/mod.rs`).

## Testing conventions

- `tests/fixtures_smoke.rs` — parametric pair runner; the canonical place to add rule coverage.
- `tests/cli.rs` — spawns the built binary via `CARGO_BIN_EXE_sas-lint` and asserts stdout/stderr/exit code.
- `tests/formatter.rs`, `tests/variable_value.rs` — focused suites for the formatter and the CSV-driven rule.
- Fixtures with non-UTF-8 bytes live under `tests/fixtures/encoding_issues_canonical/` and `tests/fixtures/lints/encoding_issues/` — touch carefully, the bytes are the test.

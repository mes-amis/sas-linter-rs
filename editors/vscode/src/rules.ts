/**
 * Autofix-capable rule ids. Kept in lockstep with `src/rules/mod.rs::all_metas()`
 * in the Rust crate — a rule's `supports_autofix: true` belongs here.
 *
 * Source of truth: run `sas-lint --list-rules` and look for the `[autofix]` mark.
 */
export const AUTOFIX_RULES: ReadonlyArray<string> = [
  "trailing_whitespace",
  "tab_expansion",
  "source_headers",
  "line_endings",
  "encoding_issues",
  "missing_assignment_semicolon",
  "unterminated_comment",
  "inconsistent_variable_case",
  "malformed_label_statement",
];

export function isAutofixable(ruleId: string): boolean {
  return AUTOFIX_RULES.includes(ruleId);
}

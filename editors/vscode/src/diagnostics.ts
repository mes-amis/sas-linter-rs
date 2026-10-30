import * as path from "node:path";
import * as vscode from "vscode";

import { resolveBinary } from "./binary";
import {
  cleanupTemp,
  configArgs,
  runBinary,
  workspaceRootFor,
  writeTempSource,
} from "./runner";

/**
 * Parses the sas-lint stdout contract:
 *
 *     <path>:<line>:<col>: [<rule_id>] <message>
 *
 * Paths may contain spaces but not `: `. The `: [` separator between column
 * and rule id anchors the regex.
 */
const FINDING_RE = /^(.+?):(\d+):(\d+): \[([A-Za-z_][A-Za-z0-9_]*)\] (.+)$/;

export interface ParsedFinding {
  path: string;
  line: number;
  column: number;
  rule: string;
  message: string;
}

function parseFindings(stdout: string): ParsedFinding[] {
  const out: ParsedFinding[] = [];
  for (const raw of stdout.split(/\r?\n/)) {
    const line = raw.trimEnd();
    if (!line) continue;
    const m = FINDING_RE.exec(line);
    if (!m) continue;
    out.push({
      path: m[1],
      line: Number(m[2]),
      column: Number(m[3]),
      rule: m[4],
      message: m[5],
    });
  }
  return out;
}

/**
 * Findings from a multi-file run, keyed by absolute path (sas-lint echoes
 * each path as given on argv, resolved here against `cwd`).
 */
export function groupFindingsByFile(stdout: string, cwd: string): Map<string, ParsedFinding[]> {
  const out = new Map<string, ParsedFinding[]>();
  for (const f of parseFindings(stdout)) {
    const key = path.resolve(cwd, f.path);
    out.set(key, [...(out.get(key) ?? []), f]);
  }
  return out;
}

function findingToDiagnostic(f: ParsedFinding, lineText: (line: number) => string): vscode.Diagnostic {
  // sas-lint emits 1-based line and 1-based column (rule code adds +1 before
  // emit; see src/finding.rs). VSCode wants 0-based.
  const line = Math.max(0, f.line - 1);
  const col = Math.max(0, f.column - 1);
  const text = lineText(line);
  const endCol = Math.min(text.length, Math.max(col + 1, text.length));
  const range = new vscode.Range(line, col, line, endCol);
  const diag = new vscode.Diagnostic(range, f.message, vscode.DiagnosticSeverity.Warning);
  diag.source = "sas-linter";
  diag.code = f.rule;
  return diag;
}

export class DiagnosticsRunner {
  private readonly collection: vscode.DiagnosticCollection;
  private readonly debounceMs = 400;
  private pending = new Map<string, NodeJS.Timeout>();

  constructor(private readonly context: vscode.ExtensionContext) {
    this.collection = vscode.languages.createDiagnosticCollection("sas-linter");
    context.subscriptions.push(this.collection);
  }

  dispose(): void {
    for (const t of this.pending.values()) clearTimeout(t);
    this.pending.clear();
  }

  scheduleRun(doc: vscode.TextDocument): void {
    if (doc.languageId !== "sas") return;
    const key = doc.uri.toString();
    const existing = this.pending.get(key);
    if (existing) clearTimeout(existing);
    const timer = setTimeout(() => {
      this.pending.delete(key);
      void this.runNow(doc);
    }, this.debounceMs);
    this.pending.set(key, timer);
  }

  clear(doc: vscode.TextDocument): void {
    this.collection.delete(doc.uri);
  }

  delete(uri: vscode.Uri): void {
    this.collection.delete(uri);
  }

  /** Publish findings for a file linted from disk rather than an open buffer. */
  set(uri: vscode.Uri, findings: ParsedFinding[], lineText: (line: number) => string): void {
    this.collection.set(uri, findings.map((f) => findingToDiagnostic(f, lineText)));
  }

  /** Drop diagnostics for every file `keep` rejects. */
  retainOnly(keep: (uri: vscode.Uri) => boolean): void {
    const drop: vscode.Uri[] = [];
    this.collection.forEach((uri) => {
      if (!keep(uri)) drop.push(uri);
    });
    for (const uri of drop) this.collection.delete(uri);
  }

  /** Re-lint every open SAS document (e.g. after the lint config changed). */
  rerunOpen(): void {
    for (const doc of vscode.workspace.textDocuments) this.scheduleRun(doc);
  }

  async runNow(doc: vscode.TextDocument): Promise<void> {
    if (doc.languageId !== "sas") return;

    let binary: string;
    try {
      binary = await resolveBinary(this.context);
    } catch (err) {
      vscode.window.showErrorMessage(`sas-linter: ${(err as Error).message}`);
      return;
    }

    const cwd = workspaceRootFor(doc);
    const tmp = writeTempSource(doc);
    try {
      const args = [...configArgs(cwd), "--no-autofix", tmp];
      const result = await runBinary(binary, args, cwd);
      if (result.code === 2) {
        // misuse — surface stderr once, then bail
        vscode.window.showErrorMessage(`sas-linter: ${result.stderr.trim() || "exit 2"}`);
        return;
      }
      // sas-lint echoes the path as given; findings about other files
      // (catalog rows) don't belong on this buffer.
      const findings = parseFindings(result.stdout).filter((f) => f.path === tmp);
      const lineText = (n: number): string => (n < doc.lineCount ? doc.lineAt(n).text : "");
      const diagnostics = findings.map((f) => findingToDiagnostic(f, lineText));
      this.collection.set(doc.uri, diagnostics);
    } finally {
      cleanupTemp(tmp);
    }
  }
}

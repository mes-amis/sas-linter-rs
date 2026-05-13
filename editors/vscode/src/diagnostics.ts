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

interface ParsedFinding {
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
      line: Number(m[2]),
      column: Number(m[3]),
      rule: m[4],
      message: m[5],
    });
  }
  return out;
}

function findingToDiagnostic(f: ParsedFinding, doc: vscode.TextDocument): vscode.Diagnostic {
  // sas-lint emits 1-based line and 1-based column (rule code adds +1 before
  // emit; see src/finding.rs). VSCode wants 0-based.
  const line = Math.max(0, f.line - 1);
  const col = Math.max(0, f.column - 1);
  const lineText = line < doc.lineCount ? doc.lineAt(line).text : "";
  const endCol = Math.min(lineText.length, Math.max(col + 1, lineText.length));
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
      const findings = parseFindings(result.stdout);
      const diagnostics = findings.map((f) => findingToDiagnostic(f, doc));
      this.collection.set(doc.uri, diagnostics);
    } finally {
      cleanupTemp(tmp);
    }
  }
}

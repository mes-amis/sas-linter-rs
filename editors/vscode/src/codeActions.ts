import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";

import { resolveBinary } from "./binary";
import { AUTOFIX_RULES, isAutofixable } from "./rules";
import {
  cleanupTemp,
  runBinary,
  workspaceRootFor,
  writeTempSource,
} from "./runner";

const FIX_COMMAND = "sasLinter.applyAutofix";

/**
 * Build a minimal YAML config that enables autofix for the given rule ids.
 * Other rules stay at their defaults (enabled, autofix off) — so they may
 * run as checks but won't rewrite the file.
 */
function buildAutofixConfig(ruleIds: string[]): string {
  const lines = ["rules:"];
  for (const id of ruleIds) {
    lines.push(`  ${id}:`);
    lines.push("    enabled: true");
    lines.push("    autofix: true");
  }
  return lines.join("\n") + "\n";
}

function writeTempConfig(yaml: string): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "sas-lint-cfg-"));
  const file = path.join(dir, "lint.yaml");
  fs.writeFileSync(file, yaml, "utf8");
  return file;
}

function cleanupTempConfig(cfg: string): void {
  try {
    fs.unlinkSync(cfg);
    fs.rmdirSync(path.dirname(cfg));
  } catch {
    // best effort
  }
}

export class SasCodeActionProvider implements vscode.CodeActionProvider {
  static readonly providedKinds = [vscode.CodeActionKind.QuickFix];

  provideCodeActions(
    doc: vscode.TextDocument,
    _range: vscode.Range | vscode.Selection,
    context: vscode.CodeActionContext,
  ): vscode.CodeAction[] {
    if (doc.languageId !== "sas") return [];

    const actions: vscode.CodeAction[] = [];
    const seen = new Set<string>();
    for (const diag of context.diagnostics) {
      if (diag.source !== "sas-linter") continue;
      const rule = typeof diag.code === "string" ? diag.code : String(diag.code ?? "");
      if (!rule || seen.has(rule) || !isAutofixable(rule)) continue;
      seen.add(rule);

      const action = new vscode.CodeAction(
        `sas-linter: autofix ${rule}`,
        vscode.CodeActionKind.QuickFix,
      );
      action.diagnostics = [diag];
      action.command = {
        title: `Autofix ${rule}`,
        command: FIX_COMMAND,
        arguments: [doc.uri, [rule]],
      };
      actions.push(action);
    }
    return actions;
  }
}

/**
 * Run sas-lint with autofix forced on for `ruleIds`. Returns the rewritten
 * source, or null when the binary made no change.
 */
async function runAutofix(
  context: vscode.ExtensionContext,
  doc: vscode.TextDocument,
  ruleIds: string[],
): Promise<string | null> {
  const binary = await resolveBinary(context);
  const cwd = workspaceRootFor(doc);
  const tmpSrc = writeTempSource(doc);
  const tmpCfg = writeTempConfig(buildAutofixConfig(ruleIds));
  try {
    const result = await runBinary(binary, ["--config", tmpCfg, tmpSrc], cwd);
    if (result.code === 2) {
      throw new Error(result.stderr.trim() || "sas-lint exit 2");
    }
    const updated = fs.readFileSync(tmpSrc, "utf8");
    return updated === doc.getText() ? null : updated;
  } finally {
    cleanupTemp(tmpSrc);
    cleanupTempConfig(tmpCfg);
  }
}

async function applyAutofixToDocument(
  context: vscode.ExtensionContext,
  doc: vscode.TextDocument,
  ruleIds: string[],
): Promise<void> {
  let updated: string | null;
  try {
    updated = await runAutofix(context, doc, ruleIds);
  } catch (err) {
    vscode.window.showErrorMessage(`sas-linter autofix: ${(err as Error).message}`);
    return;
  }
  if (!updated) return;

  const edit = new vscode.WorkspaceEdit();
  const fullRange = new vscode.Range(
    doc.positionAt(0),
    doc.positionAt(doc.getText().length),
  );
  edit.replace(doc.uri, fullRange, updated);
  await vscode.workspace.applyEdit(edit);
}

export function registerCodeActions(context: vscode.ExtensionContext): vscode.Disposable[] {
  const provider = vscode.languages.registerCodeActionsProvider(
    { language: "sas" },
    new SasCodeActionProvider(),
    { providedCodeActionKinds: SasCodeActionProvider.providedKinds },
  );

  const applyCmd = vscode.commands.registerCommand(
    FIX_COMMAND,
    async (uri: vscode.Uri, ruleIds: string[]) => {
      const doc = await vscode.workspace.openTextDocument(uri);
      await applyAutofixToDocument(context, doc, ruleIds);
    },
  );

  const fixAllCmd = vscode.commands.registerCommand("sasLinter.fixAll", async () => {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.document.languageId !== "sas") {
      vscode.window.showInformationMessage("sas-linter: open a .sas file first.");
      return;
    }
    await applyAutofixToDocument(context, editor.document, [...AUTOFIX_RULES]);
  });

  return [provider, applyCmd, fixAllCmd];
}

import * as fs from "node:fs";
import * as vscode from "vscode";

import { resolveBinary } from "./binary";
import {
  cleanupTemp,
  configArgs,
  runBinary,
  workspaceRootFor,
  writeTempSource,
} from "./runner";

export class SasFormatter implements vscode.DocumentFormattingEditProvider {
  constructor(private readonly context: vscode.ExtensionContext) {}

  async provideDocumentFormattingEdits(
    doc: vscode.TextDocument,
  ): Promise<vscode.TextEdit[]> {
    if (doc.languageId !== "sas") return [];

    let binary: string;
    try {
      binary = await resolveBinary(this.context);
    } catch (err) {
      vscode.window.showErrorMessage(`sas-linter: ${(err as Error).message}`);
      return [];
    }

    const cwd = workspaceRootFor(doc);
    const tmp = writeTempSource(doc);
    try {
      const args = ["--format", ...configArgs(cwd), tmp];
      const result = await runBinary(binary, args, cwd);
      if (result.code === 2) {
        vscode.window.showErrorMessage(`sas-linter: ${result.stderr.trim() || "exit 2"}`);
        return [];
      }
      const formatted = fs.readFileSync(tmp, "utf8");
      if (formatted === doc.getText()) return [];

      const fullRange = new vscode.Range(
        doc.positionAt(0),
        doc.positionAt(doc.getText().length),
      );
      return [vscode.TextEdit.replace(fullRange, formatted)];
    } finally {
      cleanupTemp(tmp);
    }
  }
}

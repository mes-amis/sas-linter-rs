import * as vscode from "vscode";

import { resolveBinary } from "./binary";
import { registerCodeActions } from "./codeActions";
import { DiagnosticsRunner } from "./diagnostics";
import { SasFormatter } from "./format";

export function activate(context: vscode.ExtensionContext): void {
  const diagnostics = new DiagnosticsRunner(context);
  context.subscriptions.push({ dispose: () => diagnostics.dispose() });

  // Run mode wiring.
  const runMode = (): string =>
    vscode.workspace.getConfiguration("sasLinter").get<string>("run") ?? "onSave";

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument((doc) => {
      if (runMode() !== "manual") diagnostics.scheduleRun(doc);
    }),
    vscode.workspace.onDidSaveTextDocument((doc) => {
      if (runMode() === "onSave" || runMode() === "onType") diagnostics.scheduleRun(doc);
    }),
    vscode.workspace.onDidChangeTextDocument((e) => {
      if (runMode() === "onType") diagnostics.scheduleRun(e.document);
    }),
    vscode.workspace.onDidCloseTextDocument((doc) => diagnostics.clear(doc)),
  );

  // Lint anything already open at activation time.
  for (const doc of vscode.workspace.textDocuments) {
    if (runMode() !== "manual") diagnostics.scheduleRun(doc);
  }

  // Re-resolve the binary when the pinned version or path changes —
  // downloads the newly selected release immediately (with progress
  // notification) and re-lints open SAS documents against it.
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (
        !e.affectsConfiguration("sasLinter.version") &&
        !e.affectsConfiguration("sasLinter.path")
      ) {
        return;
      }
      try {
        await resolveBinary(context);
      } catch (err) {
        vscode.window.showErrorMessage(`sas-linter: ${(err as Error).message}`);
        return;
      }
      for (const doc of vscode.workspace.textDocuments) {
        if (runMode() !== "manual") diagnostics.scheduleRun(doc);
      }
    }),
  );

  // Formatter — only register when the setting allows it. Toggling at runtime
  // takes effect on next activation; we don't watch the setting live.
  const formatEnabled = vscode.workspace
    .getConfiguration("sasLinter")
    .get<boolean>("format.enabled", true);
  if (formatEnabled) {
    context.subscriptions.push(
      vscode.languages.registerDocumentFormattingEditProvider(
        { language: "sas" },
        new SasFormatter(context),
      ),
    );
  }

  // Autofix code actions + commands.
  for (const d of registerCodeActions(context)) {
    context.subscriptions.push(d);
  }

  // Commands.
  context.subscriptions.push(
    vscode.commands.registerCommand("sasLinter.lintFile", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor || editor.document.languageId !== "sas") {
        vscode.window.showInformationMessage("sas-linter: open a .sas file first.");
        return;
      }
      await diagnostics.runNow(editor.document);
    }),
    vscode.commands.registerCommand("sasLinter.downloadBinary", async () => {
      try {
        const p = await resolveBinary(context, /* force */ true);
        vscode.window.showInformationMessage(`sas-linter: binary at ${p}`);
      } catch (err) {
        vscode.window.showErrorMessage(`sas-linter: ${(err as Error).message}`);
      }
    }),
  );
}

export function deactivate(): void {
  // Disposables registered on `context.subscriptions` are cleaned up by VSCode.
}

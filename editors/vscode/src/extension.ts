import * as vscode from "vscode";

import { resolveBinary } from "./binary";
import { registerCodeActions } from "./codeActions";
import { DiagnosticsRunner } from "./diagnostics";
import { SasFormatter } from "./format";
import { WorkspaceLinter } from "./workspace";

export function activate(context: vscode.ExtensionContext): void {
  const diagnostics = new DiagnosticsRunner(context);
  context.subscriptions.push({ dispose: () => diagnostics.dispose() });

  const workspace = new WorkspaceLinter(context, diagnostics);
  context.subscriptions.push(workspace);

  // Run mode wiring.
  const runMode = (): string =>
    vscode.workspace.getConfiguration("sasLinter").get<string>("run") ?? "onSave";
  const workspaceScope = (): boolean =>
    runMode() !== "manual" &&
    vscode.workspace.getConfiguration("sasLinter").get<string>("scope", "workspace") === "workspace";

  // Lint every SAS file in the project (per `files.associations`), or stand
  // down to open-files-only and drop the from-disk diagnostics.
  const refreshWorkspace = (): void => {
    if (workspaceScope()) {
      void workspace.lintAll();
    } else {
      workspace.stop();
      const open = new Set(vscode.workspace.textDocuments.map((d) => d.uri.toString()));
      diagnostics.retainOnly((uri) => open.has(uri.toString()));
    }
  };

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
    vscode.workspace.onDidCloseTextDocument((doc) => {
      // A project file keeps its findings once closed — re-read from disk in
      // case unsaved edits were discarded.
      if (workspaceScope() && workspace.has(doc.uri)) {
        void workspace.lintFiles([doc.uri]);
      } else {
        diagnostics.clear(doc);
      }
    }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => refreshWorkspace()),
  );

  // Lint anything already open at activation time.
  for (const doc of vscode.workspace.textDocuments) {
    if (runMode() !== "manual") diagnostics.scheduleRun(doc);
  }
  if (workspaceScope()) void workspace.lintAll();

  // Changing which files count as SAS, or how they're linted, re-runs the
  // project-wide pass.
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      const scopeChanged =
        e.affectsConfiguration("files.associations") ||
        e.affectsConfiguration("files.exclude") ||
        e.affectsConfiguration("sasLinter.scope") ||
        e.affectsConfiguration("sasLinter.run");
      if (e.affectsConfiguration("sasLinter.config")) {
        if (runMode() !== "manual") diagnostics.rerunOpen();
        refreshWorkspace();
      } else if (scopeChanged) {
        refreshWorkspace();
      }
    }),
  );

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
      if (workspaceScope()) void workspace.lintAll();
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
    vscode.commands.registerCommand("sasLinter.lintWorkspace", async () => {
      await workspace.lintAll(/* watch */ workspaceScope());
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

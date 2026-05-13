import { spawn } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";

export interface RunResult {
  code: number | null;
  stdout: string;
  stderr: string;
}

/**
 * Spawn the sas-lint binary with the given args. Resolves to stdout/stderr/code
 * once the process exits. Never throws on non-zero exit — callers decide what
 * to do with `code` (sas-lint uses 0/1/2 for clean/findings/misuse).
 */
export function runBinary(
  binary: string,
  args: string[],
  cwd: string | undefined,
): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, args, { cwd });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk: Buffer) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, stdout, stderr }));
  });
}

/**
 * Build a config-file CLI arg from the `sasLinter.config` setting. Returns an
 * empty array when the setting is blank — sas-lint falls back to its default
 * (`config/lint.yaml` relative to the cwd).
 */
export function configArgs(workspaceRoot: string | undefined): string[] {
  const cfg = vscode.workspace.getConfiguration("sasLinter");
  const configPath = cfg.get<string>("config")?.trim();
  if (!configPath) {
    return [];
  }
  const abs = path.isAbsolute(configPath) || !workspaceRoot
    ? configPath
    : path.join(workspaceRoot, configPath);
  return ["--config", abs];
}

export function workspaceRootFor(doc: vscode.TextDocument): string | undefined {
  return vscode.workspace.getWorkspaceFolder(doc.uri)?.uri.fsPath;
}

/**
 * Write the document's current text to a unique temp file. The temp file's
 * extension is preserved (`.sas`) so sas-lint treats it the same as the real
 * source.
 */
export function writeTempSource(doc: vscode.TextDocument): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "sas-lint-"));
  const file = path.join(dir, "buffer.sas");
  fs.writeFileSync(file, doc.getText(), "utf8");
  return file;
}

export function cleanupTemp(tmpFile: string): void {
  try {
    fs.unlinkSync(tmpFile);
    fs.rmdirSync(path.dirname(tmpFile));
  } catch {
    // best effort
  }
}

import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";

import { resolveBinary } from "./binary";
import { DiagnosticsRunner, groupFindingsByFile } from "./diagnostics";
import { configArgs, runBinary } from "./runner";

/** Files per sas-lint invocation — keeps argv well under Windows' 32K limit. */
const CHUNK_SIZE = 50;
const MAX_PARALLEL = 4;

/**
 * Globs that make a file "SAS" in `folder`: every `*.sas`, plus each
 * `files.associations` key mapped to the `sas` language (e.g.
 * `**\/sources/*.txt`). VSCode matches a slash-free association against the
 * basename, so those are lifted to `**\/<pattern>` for `findFiles`.
 */
export function sasGlobs(folder: vscode.WorkspaceFolder): string[] {
  const assoc =
    vscode.workspace.getConfiguration("files", folder.uri).get<Record<string, string>>("associations") ?? {};
  const globs = new Set<string>(["**/*.sas"]);
  for (const [pattern, language] of Object.entries(assoc)) {
    if (language !== "sas") continue;
    globs.add(pattern.includes("/") ? pattern : `**/${pattern}`);
  }
  return [...globs];
}

/**
 * Path of the lint config for `folder`: the `sasLinter.config` setting, or
 * sas-lint's own default (`config/lint.yaml` relative to the cwd).
 */
function configPathFor(folder: vscode.WorkspaceFolder): string {
  const [, configured] = configArgs(folder.uri.fsPath);
  return configured ?? path.join(folder.uri.fsPath, "config", "lint.yaml");
}

/**
 * Line lookup for a file that isn't open in an editor. sas-lint transcodes
 * Win-1252 sources before reporting columns, so fall back to that decoding
 * when the bytes aren't valid UTF-8.
 */
function diskLines(fsPath: string): string[] {
  let bytes: Buffer;
  try {
    bytes = fs.readFileSync(fsPath);
  } catch {
    return [];
  }
  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    text = new TextDecoder("windows-1252").decode(bytes);
  }
  return text.split(/\r?\n/);
}

function chunk<T>(items: T[], size: number): T[][] {
  const out: T[][] = [];
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size));
  return out;
}

/**
 * Lints every SAS file in the workspace from disk, so findings show up in the
 * Problems panel without opening each file. Open documents are left to
 * `DiagnosticsRunner`, which lints the live buffer instead.
 */
export class WorkspaceLinter implements vscode.Disposable {
  private watchers: vscode.Disposable[] = [];
  private known = new Set<string>();
  private generation = 0;
  private reportedError = false;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly diagnostics: DiagnosticsRunner,
  ) {}

  dispose(): void {
    this.stop();
  }

  /** Cancel any in-flight run, stop watching, and forget the file set. */
  stop(): void {
    this.generation++;
    this.disposeWatchers();
    this.known.clear();
  }

  /** Whether `uri` was in the SAS file set at the last discovery. */
  has(uri: vscode.Uri): boolean {
    return this.known.has(uri.toString());
  }

  /**
   * (Re)discover the workspace's SAS files and lint all of them. With
   * `watch`, also re-lint as those globs (or the lint config) change on disk.
   */
  async lintAll(watch = true): Promise<void> {
    const generation = ++this.generation;
    this.reportedError = false;
    if (watch) {
      this.rewatch();
    } else {
      this.disposeWatchers();
    }

    const files = new Map<string, vscode.Uri>();
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
      for (const glob of sasGlobs(folder)) {
        for (const uri of await vscode.workspace.findFiles(new vscode.RelativePattern(folder, glob))) {
          files.set(uri.toString(), uri);
        }
      }
    }
    if (generation !== this.generation) return;
    this.known = new Set(files.keys());

    // Drop diagnostics for files no longer in scope (e.g. an association was removed).
    this.diagnostics.retainOnly((uri) => files.has(uri.toString()) || this.isOpen(uri));

    const targets = [...files.values()].filter((uri) => !this.isOpen(uri));
    if (targets.length === 0) return;

    await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Window, title: `sas-linter: linting ${targets.length} files` },
      () => this.lintFiles(targets, generation),
    );
  }

  /** Lint specific on-disk files (no-op for ones open in an editor). */
  async lintFiles(uris: vscode.Uri[], generation = this.generation): Promise<void> {
    let binary: string;
    try {
      binary = await resolveBinary(this.context);
    } catch (err) {
      this.reportError((err as Error).message);
      return;
    }

    const byFolder = new Map<vscode.WorkspaceFolder, vscode.Uri[]>();
    for (const uri of uris) {
      const folder = vscode.workspace.getWorkspaceFolder(uri);
      if (!folder || this.isOpen(uri)) continue;
      byFolder.set(folder, [...(byFolder.get(folder) ?? []), uri]);
    }

    const jobs: Array<() => Promise<void>> = [];
    for (const [folder, folderUris] of byFolder) {
      for (const batch of chunk(folderUris, CHUNK_SIZE)) {
        jobs.push(() => this.lintBatch(binary, folder, batch, generation));
      }
    }
    const worker = async (): Promise<void> => {
      for (let job = jobs.shift(); job; job = jobs.shift()) {
        if (generation !== this.generation) return;
        await job();
      }
    };
    await Promise.all(Array.from({ length: MAX_PARALLEL }, worker));
  }

  private async lintBatch(
    binary: string,
    folder: vscode.WorkspaceFolder,
    batch: vscode.Uri[],
    generation: number,
  ): Promise<void> {
    const cwd = folder.uri.fsPath;
    const args = [...configArgs(cwd), "--no-autofix", ...batch.map((u) => u.fsPath)];
    let result;
    try {
      result = await runBinary(binary, args, cwd);
    } catch (err) {
      this.reportError((err as Error).message);
      return;
    }
    if (generation !== this.generation) return;
    // Per-file misses ("not a regular file") still lint the rest of the
    // batch; anything else (bad config) aborts it — surface that once per run.
    if (result.code === 2 && !/not a regular file/.test(result.stderr)) {
      this.reportError(result.stderr.trim() || "exit 2");
      return;
    }

    const found = groupFindingsByFile(result.stdout, cwd);
    for (const uri of batch) {
      // A save may have opened the file mid-run; its buffer lint wins.
      if (this.isOpen(uri) || !fs.existsSync(uri.fsPath)) {
        if (!this.isOpen(uri)) this.diagnostics.delete(uri);
        continue;
      }
      const lines = diskLines(uri.fsPath);
      this.diagnostics.set(uri, found.get(path.resolve(uri.fsPath)) ?? [], (n) => lines[n] ?? "");
    }
  }

  private isOpen(uri: vscode.Uri): boolean {
    const key = uri.toString();
    return vscode.workspace.textDocuments.some((d) => d.uri.toString() === key);
  }

  private reportError(message: string): void {
    if (this.reportedError) return;
    this.reportedError = true;
    vscode.window.showErrorMessage(`sas-linter: ${message}`);
  }

  private disposeWatchers(): void {
    for (const w of this.watchers) w.dispose();
    this.watchers = [];
  }

  private rewatch(): void {
    this.disposeWatchers();
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
      for (const glob of sasGlobs(folder)) {
        const w = vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(folder, glob));
        // Saves of open documents also fire onDidChange; lintFiles skips those.
        w.onDidCreate((uri) => {
          this.known.add(uri.toString());
          void this.lintFiles([uri]);
        });
        w.onDidChange((uri) => void this.lintFiles([uri]));
        w.onDidDelete((uri) => {
          this.known.delete(uri.toString());
          this.diagnostics.delete(uri);
        });
        this.watchers.push(w);
      }

      const configPath = configPathFor(folder);
      const w = vscode.workspace.createFileSystemWatcher(
        new vscode.RelativePattern(vscode.Uri.file(path.dirname(configPath)), path.basename(configPath)),
      );
      const relint = (): void => {
        void this.lintAll();
        this.diagnostics.rerunOpen();
      };
      w.onDidCreate(relint);
      w.onDidChange(relint);
      w.onDidDelete(relint);
      this.watchers.push(w);
    }
  }
}

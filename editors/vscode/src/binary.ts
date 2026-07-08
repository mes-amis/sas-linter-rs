import * as fs from "node:fs";
import * as path from "node:path";
import { pipeline } from "node:stream/promises";
import * as vscode from "vscode";

/**
 * Default binary version the extension expects. Bump when releasing a new
 * sas-linter-rs version that the extension depends on. Users can override it
 * per-install with the `sasLinter.version` setting — no extension release
 * needed to pick up a new linter. The asset path follows the convention from
 * the upstream README:
 *
 *   github.com/mes-amis/sas-linter-rs/releases/download/<TAG>/sas-lint-<TAG>-<TARGET>
 */
const DEFAULT_BINARY_VERSION = "v0.3.3";
const RELEASE_REPO = "mes-amis/sas-linter-rs";

/**
 * Release tag to download: the `sasLinter.version` setting when set (a bare
 * `0.3.2` is normalized to `v0.3.2`), otherwise the pinned default.
 */
function configuredVersion(): string {
  const raw = vscode.workspace.getConfiguration("sasLinter").get<string>("version")?.trim();
  if (!raw) {
    return DEFAULT_BINARY_VERSION;
  }
  return /^\d/.test(raw) ? `v${raw}` : raw;
}

// Release assets carry `.exe` on Windows; mirror that in the cache path so
// the binary stays executable when copied around.
const EXE_SUFFIX = process.platform === "win32" ? ".exe" : "";

type Target =
  | "aarch64-apple-darwin"
  | "x86_64-apple-darwin"
  | "x86_64-unknown-linux-musl"
  | "aarch64-unknown-linux-musl"
  | "x86_64-pc-windows-msvc";

function detectTarget(): Target | undefined {
  if (process.platform === "darwin") {
    return process.arch === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
  }
  if (process.platform === "linux") {
    return process.arch === "arm64" ? "aarch64-unknown-linux-musl" : "x86_64-unknown-linux-musl";
  }
  if (process.platform === "win32" && process.arch === "x64") {
    return "x86_64-pc-windows-msvc";
  }
  return undefined;
}

function cachedBinaryPath(context: vscode.ExtensionContext, version: string): string {
  const dir = context.globalStorageUri.fsPath;
  return path.join(dir, "bin", `sas-lint-${version}${EXE_SUFFIX}`);
}

async function downloadBinary(
  context: vscode.ExtensionContext,
  target: Target,
  version: string,
): Promise<string> {
  const dest = cachedBinaryPath(context, version);
  fs.mkdirSync(path.dirname(dest), { recursive: true });

  const url = `https://github.com/${RELEASE_REPO}/releases/download/${version}/sas-lint-${version}-${target}${EXE_SUFFIX}`;
  const tmp = `${dest}.tmp`;

  await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: `Downloading sas-lint ${version} (${target})`,
    },
    async () => {
      const res = await fetch(url, { redirect: "follow" });
      if (!res.ok || !res.body) {
        throw new Error(`sas-lint download failed: HTTP ${res.status} from ${url}`);
      }
      // Node's fetch returns a web stream; pipeline accepts AsyncIterable.
      await pipeline(res.body as unknown as NodeJS.ReadableStream, fs.createWriteStream(tmp));
      fs.chmodSync(tmp, 0o755);
      fs.renameSync(tmp, dest);
    },
  );

  return dest;
}

/**
 * Resolve the sas-lint binary path. Precedence:
 *   1. `sasLinter.path` setting (absolute path the user configured)
 *   2. Previously cached download in globalStorage (keyed by the
 *      `sasLinter.version` setting, or the pinned default when unset)
 *   3. Auto-download from GitHub releases
 *
 * Throws with a user-facing message on unsupported platforms or download
 * failure. The caller is expected to surface this via `vscode.window.showError`.
 */
export async function resolveBinary(
  context: vscode.ExtensionContext,
  force = false,
): Promise<string> {
  const cfg = vscode.workspace.getConfiguration("sasLinter");
  const override = cfg.get<string>("path");
  if (!force && override && override.trim().length > 0) {
    if (!fs.existsSync(override)) {
      throw new Error(`sasLinter.path is set but the file does not exist: ${override}`);
    }
    return override;
  }

  const version = configuredVersion();
  const cached = cachedBinaryPath(context, version);
  if (!force && fs.existsSync(cached)) {
    return cached;
  }

  const target = detectTarget();
  if (!target) {
    throw new Error(
      `sas-lint has no prebuilt binary for ${process.platform}-${process.arch}. ` +
        `Build from source (cargo build --release) and set sasLinter.path to the result.`,
    );
  }

  return await downloadBinary(context, target, version);
}

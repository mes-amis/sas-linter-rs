import * as fs from "node:fs";
import * as path from "node:path";
import { pipeline } from "node:stream/promises";
import * as vscode from "vscode";

/**
 * Pin the binary version the extension expects. Bump when releasing a new
 * sas-linter-rs version that the extension depends on. The asset path follows
 * the convention from the upstream README:
 *
 *   github.com/mes-amis/sas-linter-rs/releases/download/<TAG>/sas-lint-<TAG>-<TARGET>
 */
const BINARY_VERSION = "v0.2.1";
const RELEASE_REPO = "mes-amis/sas-linter-rs";

type Target =
  | "aarch64-apple-darwin"
  | "x86_64-apple-darwin"
  | "x86_64-unknown-linux-musl"
  | "aarch64-unknown-linux-musl";

function detectTarget(): Target | undefined {
  if (process.platform === "darwin") {
    return process.arch === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
  }
  if (process.platform === "linux") {
    return process.arch === "arm64" ? "aarch64-unknown-linux-musl" : "x86_64-unknown-linux-musl";
  }
  return undefined;
}

function cachedBinaryPath(context: vscode.ExtensionContext): string {
  const dir = context.globalStorageUri.fsPath;
  return path.join(dir, "bin", `sas-lint-${BINARY_VERSION}`);
}

async function downloadBinary(
  context: vscode.ExtensionContext,
  target: Target,
): Promise<string> {
  const dest = cachedBinaryPath(context);
  fs.mkdirSync(path.dirname(dest), { recursive: true });

  const url = `https://github.com/${RELEASE_REPO}/releases/download/${BINARY_VERSION}/sas-lint-${BINARY_VERSION}-${target}`;
  const tmp = `${dest}.tmp`;

  await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: `Downloading sas-lint ${BINARY_VERSION} (${target})`,
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
 *   2. Previously cached download in globalStorage
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

  const cached = cachedBinaryPath(context);
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

  return await downloadBinary(context, target);
}

// Installed and kept up to date by Warp. Local edits are overwritten.
//
// Records which session this Pi process is writing to, so Warp can reopen the right one when it
// restores the pane Pi was running in. Mirrors the `<pid>.json` files Claude Code keeps under
// `~/.claude/sessions`. Never throws into Pi and never blocks it.
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const agentDir = process.env.PI_CODING_AGENT_DIR || path.join(os.homedir(), ".pi", "agent");
const registryDir = path.join(agentDir, "warp-sessions");
const entryPath = path.join(registryDir, `${process.pid}.json`);

function sessionId(ctx: any): string {
  try {
    const file: string = ctx?.sessionManager?.getSessionFile?.() ?? "";
    if (!file) return "";
    // Session files are named `<timestamp>_<id>.jsonl`.
    const stem = path.basename(file, ".jsonl");
    const separator = stem.indexOf("_");
    return separator >= 0 ? stem.slice(separator + 1) : stem;
  } catch {
    return "";
  }
}

function record(ctx: any) {
  try {
    const id = sessionId(ctx);
    if (!id) return;
    fs.mkdirSync(registryDir, { recursive: true });
    const entry = {
      pid: process.pid,
      sessionId: id,
      cwd: process.cwd(),
      updatedAt: Date.now(),
    };
    const temporary = `${entryPath}.${process.pid}.tmp`;
    fs.writeFileSync(temporary, JSON.stringify(entry));
    fs.renameSync(temporary, entryPath);
  } catch {
    // Warp falls back to an unresumable pane; Pi itself is unaffected.
  }
}

function forget() {
  try {
    fs.rmSync(entryPath, { force: true });
  } catch {}
}

export default function (pi: ExtensionAPI) {
  pi.on("session_start", async (_event: any, ctx: any) => {
    record(ctx);
    process.once("exit", forget);
  });
  // A session replaced from inside Pi starts writing to a different file.
  pi.on("agent_start", async (_event: any, ctx: any) => record(ctx));
  pi.on("session_shutdown", async () => forget());
}

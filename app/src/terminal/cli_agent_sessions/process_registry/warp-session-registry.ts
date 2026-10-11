// Installed and kept up to date by Warp. Local edits are overwritten.
//
// Records which session this Pi process is writing to, so Warp can reopen the right one when it
// restores the pane Pi was running in. Mirrors the `<pid>.json` files Claude Code keeps under
// `~/.claude/sessions`. Also keeps a per-session event log under `warp-sessions/logs`, so a Pi
// that dies leaves a trail. Never throws into Pi and never blocks it.
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
      warpSessionUuid: process.env.WARP_TERMINAL_SESSION_UUID || undefined,
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

const logDir = path.join(registryDir, "logs");
const LOG_CAP = 5 * 1024 * 1024;
const LOG_KEEP = 1024 * 1024;
let currentSessionId = "";
let hooksInstalled = false;

function logPath(): string {
  return path.join(logDir, `${currentSessionId}.log`);
}

function truncateIfOversized(file: string) {
  const size = fs.statSync(file).size;
  if (size <= LOG_CAP) return;
  const fd = fs.openSync(file, "r");
  const tail = Buffer.alloc(LOG_KEEP);
  try {
    fs.readSync(fd, tail, 0, LOG_KEEP, size - LOG_KEEP);
  } finally {
    fs.closeSync(fd);
  }
  // The cut lands mid-line; drop the partial line so every line stays parseable JSON.
  const firstNewline = tail.indexOf(0x0a);
  fs.writeFileSync(file, firstNewline >= 0 ? tail.subarray(firstNewline + 1) : tail);
}

function log(event: string, fields: Record<string, unknown> = {}) {
  try {
    if (!currentSessionId) return;
    fs.mkdirSync(logDir, { recursive: true });
    const file = logPath();
    if (fs.existsSync(file)) truncateIfOversized(file);
    fs.appendFileSync(file, JSON.stringify({ at: Date.now(), event, ...fields }) + "\n");
  } catch {
    // The log is a courtesy to the next investigation; Pi must never pay for it.
  }
}

function errorFields(error: unknown): Record<string, unknown> {
  try {
    const e = error as { message?: string; stack?: string } | undefined;
    return { message: e?.message ?? String(error), stack: e?.stack };
  } catch {
    return {};
  }
}

function installProcessHooks() {
  // A first listener would stop Node exiting on these, so only join handlers Pi installed. A
  // skipped rejection still surfaces as an uncaught exception, which the monitor below logs.
  for (const signal of ["SIGHUP", "SIGINT", "SIGTERM"] as const) {
    if (process.listenerCount(signal) > 0) {
      process.prependListener(signal, () => log("signal", { signal }));
    }
  }
  if (process.listenerCount("unhandledRejection") > 0) {
    process.prependListener("unhandledRejection", (reason) => log("unhandled_rejection", errorFields(reason)));
  }
  process.stdin?.on?.("end", () => log("stdin_end"));
  process.stdin?.on?.("close", () => log("stdin_close"));
  process.on("uncaughtExceptionMonitor", (error) => log("uncaught_exception", errorFields(error)));
  process.on("exit", (code) => log("exit", { code }));
}

export default function (pi: ExtensionAPI) {
  pi.on("session_start", async (_event: any, ctx: any) => {
    currentSessionId = sessionId(ctx);
    record(ctx);
    process.once("exit", forget);
    if (!hooksInstalled) {
      hooksInstalled = true;
      try {
        installProcessHooks();
      } catch {}
    }
    try {
      log("session_start", { file: ctx?.sessionManager?.getSessionFile?.(), cwd: process.cwd(), pid: process.pid });
    } catch {}
  });
  // A session replaced from inside Pi starts writing to a different file.
  pi.on("agent_start", async (_event: any, ctx: any) => {
    currentSessionId = sessionId(ctx);
    record(ctx);
    log("agent_start");
  });
  pi.on("agent_end", async () => log("agent_end"));
  pi.on("message_end", async (event: any) => {
    const message = event?.message ?? {};
    log("message_end", {
      role: message.role,
      stopReason: message.stopReason,
      errorMessage: message.errorMessage,
      diagnostics: message.diagnostics,
    });
  });
  pi.on("session_shutdown", async () => {
    log("session_shutdown");
    forget();
  });
}

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

// A URL, not a path: the ESM loader refuses Windows drive paths.
const extension = new URL("./warp-session-registry.ts", import.meta.url).href;

function fakePi() {
  const handlers = new Map();
  return {
    api: { on: (event, handler) => handlers.set(event, handler) },
    fire: (event, payload, ctx) => handlers.get(event)?.(payload, ctx),
  };
}

function ctxFor(dir, id) {
  return { sessionManager: { getSessionFile: () => join(dir, `2026-10-07T00-00-00-000Z_${id}.jsonl`) } };
}

test("a turn's outcome is logged with its stop reason and error", async () => {
  const agentDir = mkdtempSync(join(tmpdir(), "pi-agent-"));
  process.env.PI_CODING_AGENT_DIR = agentDir;
  const { default: register } = await import(`${extension}?${Math.random()}`);
  const pi = fakePi();
  register(pi.api);
  await pi.fire("session_start", {}, ctxFor(agentDir, "sess-1"));
  await pi.fire(
    "message_end",
    { message: { role: "assistant", stopReason: "error", errorMessage: "WebSocket idle timeout after 300000ms",
      diagnostics: [{ type: "provider_transport_failure" }] } },
    ctxFor(agentDir, "sess-1"),
  );
  const log = readFileSync(join(agentDir, "warp-sessions", "logs", "sess-1.log"), "utf8").trim().split("\n").map(JSON.parse);
  assert.equal(log[0].event, "session_start");
  assert.equal(log[1].event, "message_end");
  assert.equal(log[1].stopReason, "error");
  assert.equal(log[1].errorMessage, "WebSocket idle timeout after 300000ms");
  assert.deepEqual(log[1].diagnostics, [{ type: "provider_transport_failure" }]);
  assert.equal(log[1].role, "assistant");
});

test("an uncaught exception is logged before the process dies", () => {
  const agentDir = mkdtempSync(join(tmpdir(), "pi-agent-"));
  const script = `
    const { default: register } = await import(${JSON.stringify(extension)});
    const handlers = new Map();
    register({ on: (e, h) => handlers.set(e, h) });
    await handlers.get("session_start")({}, { sessionManager: { getSessionFile: () => "x_sess-2.jsonl" } });
    setTimeout(() => { throw new Error("WebSocket idle timeout after 300000ms"); }, 0);
  `;
  const result = spawnSync(process.execPath, ["--input-type=module", "-e", script], {
    env: { ...process.env, PI_CODING_AGENT_DIR: agentDir },
    encoding: "utf8",
  });
  assert.notEqual(result.status, 0, "the exception must still end the process");
  const log = readFileSync(join(agentDir, "warp-sessions", "logs", "sess-2.log"), "utf8").trim().split("\n").map(JSON.parse);
  const crash = log.find((line) => line.event === "uncaught_exception");
  assert.ok(crash, "crash line present");
  assert.match(crash.message, /idle timeout/);
  assert.ok(log.some((line) => line.event === "exit" && line.code !== 0));
});

test("a log past the cap keeps only its tail", async () => {
  const agentDir = mkdtempSync(join(tmpdir(), "pi-agent-"));
  process.env.PI_CODING_AGENT_DIR = agentDir;
  const logs = join(agentDir, "warp-sessions", "logs");
  mkdirSync(logs, { recursive: true });
  writeFileSync(join(logs, "sess-3.log"), "x".repeat(5 * 1024 * 1024 + 1) + "\n");
  const { default: register } = await import(`${extension}?${Math.random()}`);
  const pi = fakePi();
  register(pi.api);
  await pi.fire("session_start", {}, ctxFor(agentDir, "sess-3"));
  const size = readFileSync(join(logs, "sess-3.log")).length;
  assert.ok(size <= 1024 * 1024 + 512, `truncated to about 1 MB, got ${size}`);
});

test("nothing is written when the session id is unknown", async () => {
  const agentDir = mkdtempSync(join(tmpdir(), "pi-agent-"));
  process.env.PI_CODING_AGENT_DIR = agentDir;
  const { default: register } = await import(`${extension}?${Math.random()}`);
  const pi = fakePi();
  register(pi.api);
  await pi.fire("session_start", {}, { sessionManager: { getSessionFile: () => "" } });
  assert.ok(!existsSync(join(agentDir, "warp-sessions", "logs")));
});

test("an unhandled rejection still ends the process and is logged as the crash", () => {
  const agentDir = mkdtempSync(join(tmpdir(), "pi-agent-"));
  const script = `
    const { default: register } = await import(${JSON.stringify(extension)});
    const handlers = new Map();
    register({ on: (e, h) => handlers.set(e, h) });
    await handlers.get("session_start")({}, { sessionManager: { getSessionFile: () => "x_sess-4.jsonl" } });
    Promise.reject(new Error("boom"));
  `;
  const result = spawnSync(process.execPath, ["--input-type=module", "-e", script], {
    env: { ...process.env, PI_CODING_AGENT_DIR: agentDir },
    encoding: "utf8",
  });
  assert.notEqual(result.status, 0, "the rejection must still end the process");
  const log = readFileSync(join(agentDir, "warp-sessions", "logs", "sess-4.log"), "utf8").trim().split("\n").map(JSON.parse);
  const crash = log.find((line) => line.event === "uncaught_exception");
  assert.ok(crash, "crash line present");
  assert.match(crash.message, /boom/);
});

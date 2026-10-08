# Kimi Code and DeepSeek Harness agents — Plan C (dashboard)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The dashboard names Kimi Code and DeepSeek Harness runs and sessions correctly: run display names say "Kimi" / "DeepSeek", the Agent Sessions page shows brand-coloured badges for both, and hook-reported session ids prefixed `kimi-` / `dsh-` deduplicate like the other agents.

**Architecture:** Three small, independent edits in the existing TypeScript backend (`server/agent-control.ts`, `server/app.ts`) and React page (`src/pages/AgentRunners.tsx`). The two backend helpers become named exports so the existing hermetic check script `scripts/check-agent-control.mjs` can test them directly. The backend contract stays the same; AgentCtrl (Plan B) already reports `backend: "kimi"` / `"dsh"`.

**Tech Stack:** TypeScript (Node server in `server/`, compiled by `yarn build:server` to `dist/server/`), React 19 + Tailwind 4 in `src/`, Node `assert` check scripts, yarn 4.

Spec: `T:/projects/dev-ops/warp-kimi-dsh/docs/superpowers/specs/2026-10-08-kimi-dsh-agents-design.md`, part C.

## Global Constraints

From the spec, verbatim:

- `server/agent-control.ts` `backendLabel`: `kimi: "Kimi"`, `dsh: "DeepSeek"`.
- `src/pages/AgentRunners.tsx` `AgentBadge` colours (Kimi blue `#1783FF`, DeepSeek indigo `#4D6BFE`); page text lists all five agents.
- `server/app.ts` `canonicalAgentRunnerSessionId` prefix regex adds `kimi|dsh`.
- `scripts/check-agent-control.mjs`: display-name cases for both.
- C needs B: merge only after Plan B is on staging.

Repository rules:

- Worktree `T:/projects/SOTZ3N/stoz3n-agent-dash-kimi-dsh`, branch `feat/kimi-dsh-agents` (already created off `origin/dev` cfc6404). Remote `origin` is Gitea (`https://git-dev.obachan.dev/stozn/stoz3n-agent-dash.git`); `gitlab-archive` is fetch-only.
- Stage explicit paths only; never `git add -A` or `git add .`.
- Every commit message ends with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- After every task: `yarn check` (both tsconfigs), `yarn lint`, and `yarn test:agent-control` (runs `yarn build:server` first) pass. CI (`.gitea/workflows/deploy.yml`) runs `yarn lint`, `yarn check`, `yarn build:server` on PRs; `yarn build` on push.
- Merging to `dev` deploys the staging dashboard. The merge needs the user's explicit go-ahead (Task 4).

Setup (once): `cd T:/projects/SOTZ3N/stoz3n-agent-dash-kimi-dsh && yarn install --immutable`.

## File map

| File | Change |
|---|---|
| `server/agent-control.ts:604-615` | `backendLabel` gains `kimi`, `dsh`; exported |
| `server/app.ts:9266-9268` | `canonicalAgentRunnerSessionId` strips `kimi-` / `dsh-`; exported |
| `src/pages/AgentRunners.tsx:38-46, 204` | `AgentBadge` brand colours and labels; page text |
| `scripts/check-agent-control.mjs` | checks for both helpers |

---

### Task 1: Run display names for Kimi and DeepSeek

**Files:**
- Modify: `server/agent-control.ts:604-615`
- Test: `scripts/check-agent-control.mjs` (import at :10, new check after `runs expose repo_url and branch` ~:631-642)

**Interfaces:**
- Produces: `export function backendLabel(value: unknown): string`. Returns `"Kimi"` for `kimi`, `"DeepSeek"` for `dsh` (case- and whitespace-insensitive); the existing labels are unchanged and unknown values still give `"Agent"`.

- [ ] **Step 1: Write the failing check**

In `scripts/check-agent-control.mjs`, change the import on line 10 to:

```js
import { agentControlError, backendLabel, handleAgentControlApi } from "../dist/server/agent-control.js";
```

Add after the `runs expose repo_url and branch` check:

```js
await check("backend labels name every AgentCtrl agent", async () => {
  assert.equal(backendLabel("kimi"), "Kimi");
  assert.equal(backendLabel(" DSH "), "DeepSeek");
  assert.equal(backendLabel("pi"), "Pi");
  assert.equal(backendLabel("codex"), "Codex");
  assert.equal(backendLabel("claude"), "Claude");
  assert.equal(backendLabel("deepseek"), "Agent");
  assert.equal(backendLabel(undefined), "Agent");
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `yarn test:agent-control`
Expected: the script aborts at import with `SyntaxError: The requested module '../dist/server/agent-control.js' does not provide an export named 'backendLabel'`.

- [ ] **Step 3: Implement**

Replace `server/agent-control.ts:604-615` with:

```ts
export function backendLabel(value: unknown): string {
  const backend = String(value ?? "").trim().toLowerCase();
  const known: Record<string, string> = {
    claude: "Claude",
    codex: "Codex",
    dsh: "DeepSeek",
    gemini: "Gemini",
    kimi: "Kimi",
    openclaw: "OpenClaw",
    opencode: "OpenCode",
    pi: "Pi",
  };
  return known[backend] ?? "Agent";
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `yarn test:agent-control`
Expected: `ok  backend labels name every AgentCtrl agent` and the final `agent-control checks passed (N)` line, exit 0.

- [ ] **Step 5: Typecheck and lint**

Run: `yarn check && yarn lint`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add server/agent-control.ts scripts/check-agent-control.mjs
git commit -m "feat(agent-control): name Kimi and DeepSeek runs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Deduplicate `kimi-` / `dsh-` prefixed agent-runner session ids

**Files:**
- Modify: `server/app.ts:9266-9268`
- Test: `scripts/check-agent-control.mjs` (the `await import("../dist/server/app.js")` at :316, new check right after it)

**Interfaces:**
- Produces: `export function canonicalAgentRunnerSessionId(value: unknown): string`. It strips one leading `claude-`, `pi-`, `codex-`, `kimi-` or `dsh-`. Kimi's own ids (`session_<uuid>`) and dsh web ids (`session-<uuid>`) contain no such prefix and stay unchanged.

- [ ] **Step 1: Write the failing check**

Change line 316 of `scripts/check-agent-control.mjs` to:

```js
const { canonicalAgentRunnerSessionId, handleDashboardRequest } = await import("../dist/server/app.js");
```

Add directly after it:

```js
await check("agent-runner session ids drop every agent prefix", async () => {
  const kimiId = "session_7d1f0b5e-3c2a-4e8f-9b61-2a4c5d6e7f80";
  const dshId = "2f0c7a52-0d55-4c55-9e43-7c3a7f2b1d10";
  assert.equal(canonicalAgentRunnerSessionId(`kimi-${kimiId}`), kimiId);
  assert.equal(canonicalAgentRunnerSessionId(kimiId), kimiId);
  assert.equal(canonicalAgentRunnerSessionId(`dsh-${dshId}`), dshId);
  assert.equal(canonicalAgentRunnerSessionId(`session-${dshId}`), `session-${dshId}`);
  assert.equal(canonicalAgentRunnerSessionId("pi-abc"), "abc");
  assert.equal(canonicalAgentRunnerSessionId("codex-abc"), "abc");
  assert.equal(canonicalAgentRunnerSessionId("claude-abc"), "abc");
  assert.equal(canonicalAgentRunnerSessionId(null), "");
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `yarn test:agent-control`
Expected: FAIL in the new check (`canonicalAgentRunnerSessionId is not a function`), exit 1.

- [ ] **Step 3: Implement**

Replace `server/app.ts:9266-9268` with:

```ts
export function canonicalAgentRunnerSessionId(value: unknown): string {
  return String(value ?? "").replace(/^(claude|pi|codex|kimi|dsh)-/, "");
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `yarn test:agent-control`
Expected: `ok  agent-runner session ids drop every agent prefix`, all checks pass, exit 0.

- [ ] **Step 5: Typecheck and lint**

Run: `yarn check && yarn lint`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add server/app.ts scripts/check-agent-control.mjs
git commit -m "feat(agent-runners): dedupe kimi- and dsh- prefixed session ids

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Agent Sessions badges and page text

**Files:**
- Modify: `src/pages/AgentRunners.tsx:38-46` (`AgentBadge`), `:204` (page description)

**Interfaces:**
- Consumes: `session.agent` values as stoz3n-hooks reports them (`kimi`, `dsh`, plus the existing `claude`, `pi`, `codex`).
- Produces: badge label "Kimi" in `#1783FF`, "DeepSeek" in `#4D6BFE`; other agents unchanged.

There is no component test harness for this page (`CLAUDE.md`: no unit test runner; the Playwright harnesses cover other pages). This task is verified by typecheck, lint, the client build and a visual check.

- [ ] **Step 1: Implement `AgentBadge`**

Replace lines 38-46 with:

```tsx
/** Agents with a brand colour Tailwind's palette lacks, and a display label unlike their id. */
const AGENT_BRANDS: Record<string, { label: string; className: string }> = {
  kimi: { label: "Kimi", className: "bg-[#1783FF]/10 text-[#1783FF]" },
  dsh: { label: "DeepSeek", className: "bg-[#4D6BFE]/10 text-[#4D6BFE]" },
};

function AgentBadge({ agent }: { agent: string }) {
  const value = agent || "claude";
  const key = value.toLowerCase();
  const brand = AGENT_BRANDS[key];
  const color = brand?.className
    ?? (key === "pi"
      ? "bg-violet-100 text-violet-800"
      : key === "codex"
        ? "bg-emerald-100 text-emerald-800"
        : "bg-orange-100 text-orange-800");
  return <span className={`rounded px-2 py-0.5 text-xs font-medium capitalize ${color}`}>{brand?.label ?? value}</span>;
}
```

- [ ] **Step 2: Update the page description (line 204)**

```tsx
            Cross-machine sessions reported by stoz3n-hooks for Claude, Pi, Codex, Kimi Code, and DeepSeek Harness. Process discovery is best-effort; an offline machine or unlinked PID does not prove that an agent has stopped.
```

- [ ] **Step 3: Typecheck, lint, client build**

Run: `yarn check && yarn lint && yarn build:client`
Expected: no errors; `build/` is produced. Confirm that Tailwind generated the arbitrary-value classes:

```bash
grep -o "1783ff\|4d6bfe" build/assets/*.css | sort -u
```

Expected: both hex values (lower-case) appear.

- [ ] **Step 4: Visual check**

Run `yarn dev` with the backend pointed at staging data (see `CLAUDE.md` "Commands"), open `/agent-runners`, and confirm a Kimi or DeepSeek session (if staging has one yet) shows the coloured badge. If staging has no such session yet, record "not visually verified, no kimi/dsh session reported yet" in the task report rather than faking data.

- [ ] **Step 5: Commit**

```bash
git add src/pages/AgentRunners.tsx
git commit -m "feat(agent-runners): Kimi and DeepSeek badges; list all five agents

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Push, PR, staging rollout (requires the user's go-ahead)

**Files:** none.

- [ ] **Step 1: Final local gate**

```bash
cd T:/projects/SOTZ3N/stoz3n-agent-dash-kimi-dsh
yarn install --immutable
yarn lint
yarn check
yarn build
yarn test:agent-control
git status --short
```

Expected: all pass; nothing uncommitted from this plan.

- [ ] **Step 2: Push and open the PR into `dev`**

```bash
git push -u origin feat/kimi-dsh-agents
tea pulls create --repo stozn/stoz3n-agent-dash --base dev --head feat/kimi-dsh-agents \
  --title "Kimi Code and DeepSeek Harness in Agent Control and Agent Sessions" \
  --description "$(cat <<'EOF'
Run display names say Kimi / DeepSeek, Agent Sessions shows brand badges for
both, and kimi-/dsh- prefixed hook session ids dedupe like the other agents.
Needs the stoz3n-agent-ctrl kimi/dsh backends (Plan B) on staging first.

Spec: warp docs/superpowers/specs/2026-10-08-kimi-dsh-agents-design.md (part C)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Expected: a PR URL; its CI run (lint, check, build:server) passes.

- [ ] **Step 3: STOP — ask the user for the go-ahead to merge**

Preconditions to state: Plan B is merged and deployed on staging (its Task 8 smoke passed); this PR's CI is green. Merging into `dev` deploys the staging dashboard. Do not merge without an explicit yes.

- [ ] **Step 4: After the merge, verify on staging**

Watch the dashboard deploy workflow in Gitea until it finishes. Then open the staging dashboard's Agent Control page: a kimi or dsh run started in Plan B Task 8 lists as `<repo> · Kimi · <id>` / `<repo> · DeepSeek · <id>`. Report the result.

---

## Spec gaps found while planning

1. `AgentBadge` renders the raw agent id, so `dsh` would show as "Dsh". The plan gives both new badges a label ("Kimi", "DeepSeek") matching `backendLabel`. The other agents are unchanged.
2. `backendLabel` and `canonicalAgentRunnerSessionId` are module-private. The plan exports them so the existing check script can test them, because fixture runs for kimi/dsh would change the counts other checks assert.
3. The brand colours as text on a 10 % tint of themselves have low contrast (about 3.5:1 for `#1783FF`). The spec fixes the colours; a darker text shade would need the spec changed.
4. The Agent Sessions page shows what stoz3n-hooks reports. Whether stoz3n-hooks reports Kimi or dsh sessions at all is outside this spec. The page text now names them anyway, as the spec asks.

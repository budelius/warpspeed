// Cordis plugin: reports this DeepSeek Harness session's progress to the WarpSpeed pane it runs in,
// through `$WARP_AGENT_EVENT_BIN agent-event --format protocol`. Outside a WarpSpeed pane the
// variable is unset and the plugin does nothing.

import { type ChildProcess, spawn } from 'node:child_process'
import { readFileSync } from 'node:fs'

import { dshHome, notification, type Notification, type SessionHeader, textOf, transcriptPath } from './protocol.ts'

export const name = 'dsh-warp'

const VERSION: string = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).version

/** How long one delivery may take before the next is sent anyway; `agent-event` gives up after 2 s. */
const DELIVERY_TIMEOUT_MS = 3000

interface Session {
  readonly header: SessionHeader
}
interface Agent {
  readonly session: Session
}
interface UserMessage {
  readonly content?: unknown
  readonly source?: { readonly kind?: string }
}
interface ToolExecution {
  readonly name: string
  readonly arguments?: unknown
  readonly agent?: Agent
}
interface SessionEvent {
  readonly type: string
  readonly data?: { readonly toolName?: string; readonly reason?: string }
}

/** The part of Cordis' `Context` this plugin uses (`@deepseek-ai/cordis` 4.0.5-alpha.1). */
export interface Context {
  on(name: 'agent/created', listener: (payload: { agent: Agent; source: string }) => undefined): unknown
  on(name: 'agent/inbox/claimed', listener: (payload: { agent: Agent; message: UserMessage; turn: number }) => void): unknown
  on(name: 'tools/result', listener: (exec: ToolExecution, result: unknown) => void): unknown
  on(name: 'session/event', listener: (session: Session, event: SessionEvent) => void): unknown
  on(name: 'agent/turn-stopping', listener: (payload: { agent: Agent; turn: number }) => void): unknown
  on(name: 'agent/status', listener: (payload: { agent: Agent; status: 'idle' | 'running' }) => void): unknown
}

/** Reports one event of a session. */
export type Report = (header: SessionHeader, event: string, extra?: Partial<Notification>) => void

export function apply(ctx: Context): void {
  const binary = process.env.WARP_AGENT_EVENT_BIN
  if (!binary) return
  const send = serialSender(binary)
  subscribe(ctx, (header, event, extra) => send(notification(event, header, VERSION, extra)))
}

/** Turns the dsh events this plugin follows into `report` calls; `home` is dsh's data root. */
export function subscribe(ctx: Context, report: Report, home: string = dshHome()): void {
  const forSession: Report = (header, event, extra) => {
    // A subagent's session is not the one the pane shows.
    if (header.origin !== 'subagent') report(header, event, extra)
  }

  ctx.on('agent/created', ({ agent, source }) => {
    if (source === 'startup' || source === 'resume') {
      const header = agent.session.header
      forSession(header, 'session_start', { cwd: header.cwd, transcript_path: transcriptPath(header, home) })
    }
    return undefined
  })
  ctx.on('agent/inbox/claimed', ({ agent, message }) => {
    if (message.source?.kind === 'user') {
      forSession(agent.session.header, 'prompt_submit', { query: textOf(message.content) })
    }
  })
  ctx.on('tools/result', (exec) => {
    if (exec.agent) {
      forSession(exec.agent.session.header, 'tool_complete', { tool_name: exec.name, tool_input: exec.arguments })
    }
  })
  ctx.on('session/event', (session, event) => {
    if (event.type === 'approval/asked') {
      const toolName = event.data?.toolName
      forSession(session.header, 'permission_request', {
        tool_name: toolName,
        summary: event.data?.reason ?? (toolName ? `Approve ${toolName}?` : undefined),
      })
    } else if (event.type === 'approval/decided') {
      forSession(session.header, 'permission_replied')
    }
  })
  ctx.on('agent/turn-stopping', ({ agent }) => forSession(agent.session.header, 'stop'))
  ctx.on('agent/status', ({ agent, status }) => {
    if (status === 'idle') forSession(agent.session.header, 'idle_prompt')
  })
}

/** Sends notifications one at a time and in order, so a `stop` never overtakes its `prompt_submit`. */
function serialSender(binary: string): (event: Notification) => void {
  let queue: Promise<void> = Promise.resolve()
  return (event) => {
    queue = queue.then(() => deliver(binary, event))
  }
}

function deliver(binary: string, event: Notification): Promise<void> {
  return new Promise((resolve) => {
    let child: ChildProcess
    try {
      child = spawn(binary, ['agent-event', '--format', 'protocol'], { stdio: ['pipe', 'ignore', 'ignore'], windowsHide: true })
    } catch {
      resolve()
      return
    }
    const timer = setTimeout(() => {
      child.kill()
      finish()
    }, DELIVERY_TIMEOUT_MS)
    function finish() {
      clearTimeout(timer)
      resolve()
    }
    child.once('error', finish)
    child.once('exit', finish)
    child.stdin?.once('error', () => {})
    child.stdin?.end(JSON.stringify(event))
  })
}

import assert from 'node:assert/strict'
import { join } from 'node:path'
import { test } from 'node:test'

import { type Context, deliver, serialSender, subscribe } from './index.ts'
import type { Notification } from './protocol.ts'

type Listener = (...args: any[]) => unknown

/** A stand-in for Cordis' context that keeps the listeners, and what the plugin reports. */
function harness(throwing = false) {
  const listeners = new Map<string, Listener>()
  const ctx = {
    on: (name: string, listener: Listener) => {
      listeners.set(name, listener)
    },
  } as unknown as Context
  const reports: Array<{ id: string; event: string; extra: Record<string, unknown> }> = []
  subscribe(
    ctx,
    (header, event, extra = {}) => {
      if (throwing) throw new Error('report failed')
      reports.push({ id: header.id, event, extra })
    },
    '/home/me/.dsh',
  )
  const emit = (name: string, ...args: unknown[]) => listeners.get(name)!(...args)
  return { emit, reports }
}

const header = { id: 'session-1', cwd: '/work/repo', version: 4 }
const agent = { session: { header } }
const subagent = { session: { header: { ...header, id: 'session-2', origin: 'subagent' } } }

test('a started or resumed session reports session_start with its log', () => {
  const { emit, reports } = harness()
  assert.equal(emit('agent/created', { agent, source: 'startup' }), undefined)
  emit('agent/created', { agent, source: 'resume' })
  emit('agent/created', { agent, source: 'compact' })
  emit('agent/created', { agent: subagent, source: 'startup' })
  const transcript = join('/home/me/.dsh', 'sessions', '--work-repo--', 'session-1', 'session.v4.jsonl.zstd')
  assert.deepEqual(reports, [
    { id: 'session-1', event: 'session_start', extra: { cwd: '/work/repo', transcript_path: transcript } },
    { id: 'session-1', event: 'session_start', extra: { cwd: '/work/repo', transcript_path: transcript } },
  ])
})

test('prompts, tools, approvals, turn ends and idling are reported', () => {
  const { emit, reports } = harness()
  emit('agent/inbox/claimed', { agent, turn: 1, message: { content: [{ type: 'text', text: 'Fix it' }], source: { kind: 'user' } } })
  emit('agent/inbox/claimed', { agent, turn: 1, message: { content: [{ type: 'text', text: '<env>' }], source: { kind: 'runtime-context' } } })
  emit('tools/result', { name: 'bash', arguments: { command: 'ls' }, agent }, { isError: false })
  emit('tools/result', { name: 'bash', arguments: {} }, { isError: false })
  emit('session/event', agent.session, { type: 'approval/asked', data: { id: 'a1', toolName: 'bash', reason: 'Run ls?' } })
  emit('session/event', agent.session, { type: 'approval/asked', data: { id: 'a2', toolName: 'write' } })
  emit('session/event', agent.session, { type: 'approval/decided', data: { id: 'a1', outcome: 'allowed-once' } })
  emit('session/event', agent.session, { type: 'user/message', data: {} })
  emit('agent/turn-stopping', { agent, turn: 1 })
  emit('agent/status', { agent, status: 'running' })
  emit('agent/status', { agent, status: 'idle' })
  emit('agent/turn-stopping', { agent: subagent, turn: 1 })
  assert.deepEqual(
    reports.map(({ event, extra }) => ({ event, extra })),
    [
      { event: 'prompt_submit', extra: { query: 'Fix it' } },
      { event: 'tool_complete', extra: { tool_name: 'bash', tool_input: { command: 'ls' } } },
      { event: 'permission_request', extra: { tool_name: 'bash', summary: 'Run ls?' } },
      { event: 'permission_request', extra: { tool_name: 'write', summary: 'Approve write?' } },
      { event: 'permission_replied', extra: {} },
      { event: 'stop', extra: {} },
      { event: 'idle_prompt', extra: {} },
    ],
  )
})

test('no listener throws into dsh', () => {
  const { emit, reports } = harness()
  const unnamed = { session: { header: { id: '', cwd: '/work/repo', version: 4 } } }
  assert.equal(emit('agent/created', { agent: unnamed, source: 'startup' }), undefined)
  assert.deepEqual(reports, [])

  const failing = harness(true)
  assert.equal(failing.emit('agent/created', { agent, source: 'startup' }), undefined)
  failing.emit('agent/inbox/claimed', { agent, turn: 1, message: { content: 'hi', source: { kind: 'user' } } })
  failing.emit('tools/result', { name: 'bash', arguments: {}, agent }, {})
  failing.emit('session/event', agent.session, { type: 'approval/decided', data: { id: 'a1' } })
  failing.emit('agent/turn-stopping', { agent, turn: 1 })
  failing.emit('agent/status', { agent, status: 'idle' })
})

test('a delivery that fails does not stop the ones after it', async () => {
  const rejections: unknown[] = []
  const onRejection = (reason: unknown) => rejections.push(reason)
  process.on('unhandledRejection', onRejection)
  try {
    const delivered: string[] = []
    const send = serialSender(async (event) => {
      delivered.push(JSON.stringify(event))
    })
    const unserializable: Notification = { v: 1, agent: 'dsh', event: 'tool_complete', tool_input: { size: 1n } }
    const normal: Notification = { v: 1, agent: 'dsh', event: 'stop' }
    send(unserializable)
    await send(normal)
    await new Promise((resolve) => setImmediate(resolve))
    assert.deepEqual(delivered, [JSON.stringify(normal)])
    assert.deepEqual(rejections, [])
  } finally {
    process.off('unhandledRejection', onRejection)
  }
})

test('deliver resolves without spawning when the event cannot be serialised', async () => {
  const circular: Record<string, unknown> = {}
  circular.self = circular
  await deliver('warpspeed-binary-that-does-not-exist', { v: 1, agent: 'dsh', event: 'tool_complete', tool_input: circular })
})

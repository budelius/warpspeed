// The pure half of the plugin: what Warp's CLI agent protocol expects, and where dsh keeps a session.

import { homedir } from 'node:os'
import { join } from 'node:path'

export const AGENT = 'dsh'
export const PROTOCOL_VERSION = 1

/** A Warp CLI agent protocol v1 notification, as `warpspeed agent-event --format protocol` reads it. */
export interface Notification {
  v: number
  agent: string
  event: string
  session_id?: string
  cwd?: string
  query?: string
  transcript_path?: string
  summary?: string
  tool_name?: string
  tool_input?: unknown
  plugin_version?: string
}

/** The fields of dsh's `SessionHeader` this plugin reads. */
export interface SessionHeader {
  readonly id: string
  readonly cwd?: string
  readonly version?: number
  readonly origin?: string
}

/** dsh's `projectKey` (session-persistence-jsonl/src/format.ts:225-245), verbatim in behaviour. */
export function projectKey(cwd: string): string {
  if (cwd.length === 0) throw new Error('cannot encode an empty project path')
  let readable = ''
  let separatorRun = false
  for (let i = 0; i < cwd.length; i++) {
    const code = cwd.charCodeAt(i)
    const ch = String.fromCharCode(code)
    if (ch === '/' || ch === '\\' || ch === ':') {
      if (!separatorRun) readable += '-'
      separatorRun = true
    } else if (ch !== '~' && /^[A-Za-z0-9._-]$/.test(ch)) {
      readable += ch
      separatorRun = false
    } else {
      readable += '~' + code.toString(16).toUpperCase().padStart(4, '0')
      separatorRun = false
    }
  }
  const slug = readable.replace(/^-+/, '') || 'root'
  return `--${slug.slice(0, 251)}--`
}

/** dsh's `encodeSegment` (session-persistence-jsonl/src/format.ts:199-214), verbatim in behaviour. */
export function encodeSegment(raw: string): string {
  if (raw.length === 0) throw new Error('cannot encode an empty path segment')
  if (raw === '.') return '~002E'
  if (raw === '..') return '~002E~002E'
  let out = ''
  for (let i = 0; i < raw.length; i++) {
    const code = raw.charCodeAt(i)
    const ch = String.fromCharCode(code)
    if (ch !== '~' && /^[A-Za-z0-9._-]$/.test(ch)) {
      out += ch
    } else {
      out += '~' + code.toString(16).toUpperCase().padStart(4, '0')
    }
  }
  return out
}

/** dsh's data root: `$DSH_HOME` unless blank, else `~/.dsh`. */
export function dshHome(env: NodeJS.ProcessEnv = process.env): string {
  const configured = env.DSH_HOME?.trim()
  return configured ? configured : join(homedir(), '.dsh')
}

/**
 * Where the base bundle's JSONL persistence writes this session's current log. The file appears
 * on the first flush, and a session without a cwd lives elsewhere (`_no-cwd`), so it has none.
 */
export function transcriptPath(header: SessionHeader, home: string): string | undefined {
  if (!header.cwd) return undefined
  return join(home, 'sessions', projectKey(header.cwd), encodeSegment(header.id), `session.v${header.version ?? 4}.jsonl.zstd`)
}

/** The text of a message's content parts, joined; `undefined` when there is none. */
export function textOf(content: unknown): string | undefined {
  if (typeof content === 'string') return content.trim() || undefined
  if (!Array.isArray(content)) return undefined
  const text = content
    .filter((part): part is { type: 'text'; text: string } => part?.type === 'text' && typeof part.text === 'string')
    .map((part) => part.text)
    .join('\n')
    .trim()
  return text || undefined
}

/** A notification for `header`'s session. Fields of `extra` that are `undefined` are left out. */
export function notification(event: string, header: SessionHeader, pluginVersion: string, extra: Partial<Notification> = {}): Notification {
  const defined = Object.fromEntries(Object.entries(extra).filter(([, value]) => value !== undefined))
  return { v: PROTOCOL_VERSION, agent: AGENT, event, session_id: header.id, plugin_version: pluginVersion, ...defined }
}

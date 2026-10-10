import assert from 'node:assert/strict'
import { join } from 'node:path'
import { test } from 'node:test'

import { dshHome, encodeSegment, notification, projectKey, textOf, transcriptPath } from './protocol.ts'

test('projectKey matches dsh', () => {
  // dsh's own vectors (session-persistence-jsonl/tests/jsonl.spec.ts:487-493), then two by hand.
  assert.equal(projectKey('/Users/qyj/work/deepseek-harness'), '--Users-qyj-work-deepseek-harness--')
  assert.equal(projectKey('/a/b-c'), projectKey('/a-b/c'))
  assert.equal(projectKey('C:\\work\\agent'), '--C-work-agent--')
  assert.equal(projectKey('/开发/~agent'), '--~5F00~53D1-~007Eagent--')
  assert.equal(projectKey('/'), '--root--')
  assert.equal(projectKey('/' + 'x'.repeat(1_000)).length, 255)
  assert.throws(() => projectKey(''), /empty project path/)
  assert.equal(projectKey('C:\\Users\\me\\proj'), '--C-Users-me-proj--')
  assert.equal(projectKey('/home/me/my project/'), '--home-me-my~0020project---')
})

test('encodeSegment matches dsh', () => {
  assert.equal(encodeSegment('..'), '~002E~002E')
  assert.equal(encodeSegment('.'), '~002E')
  assert.equal(encodeSegment('a/b'), 'a~002Fb')
  assert.equal(encodeSegment('/etc/passwd'), '~002Fetc~002Fpasswd')
  assert.equal(encodeSegment('a\u0000b'), 'a~0000b')
  assert.equal(encodeSegment('plain-ID_1.2'), 'plain-ID_1.2')
  assert.equal(encodeSegment('a~b'), 'a~007Eb')
  assert.equal(encodeSegment('session-0b6c1234-ffff-4a4a-8b8b-0123456789ab'), 'session-0b6c1234-ffff-4a4a-8b8b-0123456789ab')
})

test('dshHome honours DSH_HOME unless blank', () => {
  assert.equal(dshHome({ DSH_HOME: '/srv/dsh' }), '/srv/dsh')
  assert.notEqual(dshHome({ DSH_HOME: '   ' }), '   ')
  assert.ok(dshHome({}).endsWith('.dsh'))
})

test('transcriptPath is where dsh writes the session log', () => {
  const header = { id: 'session-1', cwd: 'C:\\Users\\me\\proj', version: 4 }
  assert.equal(
    transcriptPath(header, '/home/me/.dsh'),
    join('/home/me/.dsh', 'sessions', '--C-Users-me-proj--', 'session-1', 'session.v4.jsonl.zstd'),
  )
  assert.equal(transcriptPath({ id: 'session-1' }, '/home/me/.dsh'), undefined)
})

test('textOf joins text parts', () => {
  assert.equal(textOf([{ type: 'text', text: 'Fix the build' }, { type: 'image' }, { type: 'text', text: 'and the tests' }]), 'Fix the build\nand the tests')
  assert.equal(textOf('  plain  '), 'plain')
  assert.equal(textOf([{ type: 'image' }]), undefined)
  assert.equal(textOf(undefined), undefined)
})

test('notification is a protocol v1 event from dsh', () => {
  assert.deepEqual(notification('stop', { id: 'session-1' }, '0.1.0'), {
    v: 1,
    agent: 'dsh',
    event: 'stop',
    session_id: 'session-1',
    plugin_version: '0.1.0',
  })
  assert.deepEqual(notification('prompt_submit', { id: 'session-1' }, '0.1.0', { query: 'hi', summary: undefined }), {
    v: 1,
    agent: 'dsh',
    event: 'prompt_submit',
    session_id: 'session-1',
    plugin_version: '0.1.0',
    query: 'hi',
  })
})

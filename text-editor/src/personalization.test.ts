import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { emit } from './events.ts'
import { normalizeUrl, sessionId, toServerAction, toUserAction, userId } from './personalization.ts'
import vocabulary from './vocabulary.json'

describe('personalization client', () => {
  it('sends events in the server action shape', () => {
    const event = emit({ action: 'save', payload: { id: 'a', text: 'hi' } }, new Date(0), 'system')
    expect(toServerAction(event)).toEqual({
      user_id: userId,
      session_id: sessionId,
      action_name: 'save',
      payload: { id: 'a', text: 'hi' },
      source: 'system',
    })
  })

  it('turns suggested actions into editor actions, rejecting ones it cannot apply', () => {
    const s = (action_name: string, payload: unknown) => ({ action_name, payload, source: 'system' })
    expect(toUserAction(s('write_text', { id: 'a', text: 'x' }))).toEqual({ action: 'write_text', payload: { id: 'a', text: 'x' } })
    expect(toUserAction(s('select', { id: 'a', extra: 1 }))).toEqual({ action: 'select', payload: { id: 'a' } })
    expect(toUserAction(s('save', { id: 'a' }))).toBeNull()
    expect(toUserAction(s('rename', { id: 'a' }))).toBeNull()
    expect(toUserAction(s('create', null))).toBeNull()
  })

  it('normalizes server URLs', () => {
    expect(normalizeUrl(' localhost:8080/ ')).toBe('http://localhost:8080')
    expect(normalizeUrl('https://p13n.example.com//')).toBe('https://p13n.example.com')
  })

  it("keeps its vocabulary in sync with the server's example", () => {
    const server = JSON.parse(readFileSync(new URL('../../intelligence-engine/examples/text-editor.schema.json', import.meta.url), 'utf8'))
    expect(vocabulary).toEqual(server)
  })
})

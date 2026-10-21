import { describe, expect, it } from 'vitest'
import { emit, type UserAction } from './events.ts'
import { isDirty, reduce, snippetOf, titleOf, type State } from './notes.ts'
import { loadNotes, serialize, writeNotes } from './storage.ts'

let tick = 0
const at = () => new Date(Date.UTC(2026, 9, 20, 12, 0, tick++))
const run = (actions: UserAction[], state: State = { notes: [], selectedId: null }) =>
  actions.reduce((s, action) => reduce(s, emit(action, at())), state)

describe('reduce', () => {
  it('creates, edits, saves and selects notes', () => {
    const state = run([
      { action: 'create', payload: { id: 'a' } },
      { action: 'write_text', payload: { id: 'a', text: '# First' } },
      { action: 'create', payload: { id: 'b' } },
      { action: 'save', payload: { id: 'a', text: '# First' } },
    ])
    expect(state.notes.map((n) => n.id)).toEqual(['b', 'a'])
    expect(state.selectedId).toBe('b')
    const a = state.notes.find((n) => n.id === 'a')!
    expect(a.text).toBe('# First')
    expect(isDirty(a)).toBe(false)
    expect(run([{ action: 'select', payload: { id: 'a' } }], state).selectedId).toBe('a')
  })

  it('marks unsaved text as dirty and bumps updatedAt', () => {
    const created = run([{ action: 'create', payload: { id: 'a' } }])
    const edited = run([{ action: 'write_text', payload: { id: 'a', text: 'hi' } }], created)
    expect(isDirty(edited.notes[0])).toBe(true)
    expect(edited.notes[0].updatedAt > created.notes[0].updatedAt).toBe(true)
  })

  it('deleting the open note opens the most recently edited one', () => {
    const state = run([
      { action: 'create', payload: { id: 'a' } },
      { action: 'create', payload: { id: 'b' } },
      { action: 'create', payload: { id: 'c' } },
      { action: 'write_text', payload: { id: 'a', text: 'newest edit' } },
      { action: 'select', payload: { id: 'c' } },
      { action: 'delete', payload: { id: 'c' } },
    ])
    expect(state.notes.map((n) => n.id)).toEqual(['b', 'a'])
    expect(state.selectedId).toBe('a')
    expect(run([{ action: 'delete', payload: { id: 'a' } }, { action: 'delete', payload: { id: 'b' } }], state).selectedId).toBeNull()
  })

  it('ignores events for unknown notes and duplicate creates', () => {
    const state = run([{ action: 'create', payload: { id: 'a' } }])
    expect(run([{ action: 'select', payload: { id: 'nope' } }], state)).toBe(state)
    expect(run([{ action: 'create', payload: { id: 'a' } }], state)).toBe(state)
  })

  it('emits plain JSON events', () => {
    const event = emit({ action: 'save', payload: { id: 'a', text: 'x' } }, new Date(0))
    expect(JSON.parse(JSON.stringify(event))).toEqual({ action: 'save', payload: { id: 'a', text: 'x' }, at: '1970-01-01T00:00:00.000Z', source: 'user' })
  })
})

describe('titles', () => {
  it('uses the first two non-empty lines without markdown syntax', () => {
    expect(titleOf('\n# **Groceries**\n\n- [ ] eggs')).toBe('Groceries')
    expect(snippetOf('\n# Groceries\n\n- [ ] eggs')).toBe('eggs')
    expect(titleOf('   ')).toBe('Untitled')
  })
})

describe('storage', () => {
  const memory = (): Storage => {
    const items = new Map<string, string>()
    return {
      get length() { return items.size },
      clear: () => items.clear(),
      getItem: (key) => items.get(key) ?? null,
      key: (i) => [...items.keys()][i] ?? null,
      removeItem: (key) => void items.delete(key),
      setItem: (key, value) => void items.set(key, value),
    }
  }

  it('round-trips only saved text', () => {
    const storage = memory()
    const state = run([
      { action: 'create', payload: { id: 'a' } },
      { action: 'write_text', payload: { id: 'a', text: 'saved' } },
      { action: 'save', payload: { id: 'a', text: 'saved' } },
      { action: 'write_text', payload: { id: 'a', text: 'saved, then more' } },
    ])
    writeNotes(serialize(state.notes), storage)
    const [loaded] = loadNotes(storage)
    expect(loaded.text).toBe('saved')
    expect(isDirty(loaded)).toBe(false)
  })

  it('treats corrupt data as empty', () => {
    const storage = memory()
    storage.setItem('p13n.text-editor.notes.v1', '{not json')
    expect(loadNotes(storage)).toEqual([])
    storage.setItem('p13n.text-editor.notes.v1', JSON.stringify([{ id: 1 }, null]))
    expect(loadNotes(storage)).toEqual([])
  })
})

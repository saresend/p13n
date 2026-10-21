import { useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef } from 'react'
import { emit, newId, type UserAction } from './events.ts'
import { isDirty, reduce, sortNotes, type State } from './notes.ts'
import { loadNotes, serialize, writeNotes } from './storage.ts'

export type Handlers = {
  create: () => void
  select: (id: string) => void
  writeText: (id: string, text: string) => void
  save: (id: string) => void
  remove: (id: string) => void
}

const AUTOSAVE_MS = 800

function initialState(): State {
  const notes = loadNotes()
  return { notes, selectedId: sortNotes(notes)[0]?.id ?? null }
}

// Notes state plus one handler per user action. Each handler emits an event and applies it.
export function useNotes() {
  const [state, apply] = useReducer(reduce, undefined, initialState)
  // Handlers read the latest committed state through this ref, so they never go stale.
  const stateRef = useRef(state)
  useLayoutEffect(() => {
    stateRef.current = state
  }, [state])

  const fire = useCallback((action: UserAction) => apply(emit(action)), [])

  const handlers = useMemo<Handlers>(() => {
    const noteById = (id: string) => stateRef.current.notes.find((note) => note.id === id)
    const save = (id: string) => {
      const note = noteById(id)
      if (note && isDirty(note)) fire({ action: 'save', payload: { id, text: note.text } })
    }
    // Leaving a note saves it, so pending autosaves are never dropped.
    const leaveCurrent = () => {
      if (stateRef.current.selectedId) save(stateRef.current.selectedId)
    }
    return {
      create: () => {
        leaveCurrent()
        fire({ action: 'create', payload: { id: newId() } })
      },
      select: (id) => {
        if (id === stateRef.current.selectedId) return
        leaveCurrent()
        fire({ action: 'select', payload: { id } })
      },
      writeText: (id, text) => fire({ action: 'write_text', payload: { id, text } }),
      save,
      remove: (id) => fire({ action: 'delete', payload: { id } }),
    }
  }, [fire])

  // Persist whenever the saved snapshot changes (not on every keystroke).
  const snapshot = useMemo(() => serialize(state.notes), [state.notes])
  useEffect(() => writeNotes(snapshot), [snapshot])

  // Autosave the open note shortly after typing stops.
  const selected = state.notes.find((note) => note.id === state.selectedId) ?? null
  useEffect(() => {
    if (!selected || !isDirty(selected)) return
    const timer = setTimeout(() => handlers.save(selected.id), AUTOSAVE_MS)
    return () => clearTimeout(timer)
  }, [selected, handlers])

  // Closing the tab can't wait for a re-render, so save straight to storage.
  useEffect(() => {
    const flush = () => {
      const dirty = stateRef.current.notes.filter(isDirty)
      if (!dirty.length) return
      dirty.forEach((note) => emit({ action: 'save', payload: { id: note.id, text: note.text } }))
      writeNotes(serialize(stateRef.current.notes.map((note) => ({ ...note, savedText: note.text }))))
    }
    window.addEventListener('pagehide', flush)
    return () => window.removeEventListener('pagehide', flush)
  }, [])

  return { notes: state.notes, selected, handlers }
}

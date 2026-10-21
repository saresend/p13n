import type { UserEvent } from './events.ts'

export type Note = {
  id: string
  text: string
  // Text as of the last save; `text !== savedText` means unsaved changes.
  savedText: string
  createdAt: string
  updatedAt: string
}

export type State = { notes: Note[]; selectedId: string | null }

const update = (notes: Note[], id: string, change: (note: Note) => Note) =>
  notes.map((note) => (note.id === id ? change(note) : note))

// Pure: the state after `event`. Unknown ids are ignored, so replaying a log is always safe.
export function reduce(state: State, event: UserEvent): State {
  switch (event.action) {
    case 'create': {
      if (state.notes.some((note) => note.id === event.payload.id)) return state
      const note = { id: event.payload.id, text: '', savedText: '', createdAt: event.at, updatedAt: event.at }
      return { notes: [note, ...state.notes], selectedId: note.id }
    }
    case 'select':
      if (!state.notes.some((note) => note.id === event.payload.id)) return state
      return { ...state, selectedId: event.payload.id }
    case 'write_text': {
      const { id, text } = event.payload
      return { ...state, notes: update(state.notes, id, (note) => ({ ...note, text, updatedAt: event.at })) }
    }
    case 'save': {
      const { id, text } = event.payload
      return { ...state, notes: update(state.notes, id, (note) => ({ ...note, text, savedText: text })) }
    }
    case 'delete': {
      const notes = state.notes.filter((note) => note.id !== event.payload.id)
      const selectedId = state.selectedId === event.payload.id ? (sortNotes(notes)[0]?.id ?? null) : state.selectedId
      return { notes, selectedId }
    }
  }
}

export const isDirty = (note: Note) => note.text !== note.savedText

// Most recently edited first, like Bear.
export const sortNotes = (notes: Note[]) => [...notes].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))

const lines = (text: string) => text.split('\n').map((line) => line.trim()).filter(Boolean)
const plain = (line: string) => line.replace(/^(#{1,6}\s+|>\s*|[-*+]\s+(\[[ xX]\]\s+)?|\d+[.)]\s+)/, '').replace(/[*_`~]/g, '')

// The first non-empty line is the title; the next one is the sidebar snippet.
export const titleOf = (text: string) => plain(lines(text)[0] ?? '') || 'Untitled'
export const snippetOf = (text: string) => plain(lines(text)[1] ?? '')

export const wordCount = (text: string) => text.split(/\s+/).filter(Boolean).length

import type { Note } from './notes.ts'

// Saved notes live in localStorage until there's a server. Only saved text is written.
const KEY = 'p13n.text-editor.notes.v1'

type Stored = { id: string; text: string; createdAt: string; updatedAt: string }

const isStored = (value: unknown): value is Stored => {
  const v = value as Stored
  return typeof v === 'object' && v !== null &&
    [v.id, v.text, v.createdAt, v.updatedAt].every((field) => typeof field === 'string')
}

export function loadNotes(storage: Storage | undefined = globalThis.localStorage): Note[] {
  try {
    const parsed: unknown = JSON.parse(storage?.getItem(KEY) ?? '[]')
    if (!Array.isArray(parsed)) return []
    return parsed.filter(isStored).map((note) => ({ ...note, savedText: note.text }))
  } catch {
    return []
  }
}

export const serialize = (notes: Note[]) =>
  JSON.stringify(notes.map(({ id, savedText, createdAt, updatedAt }): Stored => ({ id, text: savedText, createdAt, updatedAt })))

export function writeNotes(serialized: string, storage: Storage | undefined = globalThis.localStorage) {
  try {
    storage?.setItem(KEY, serialized)
  } catch (error) {
    console.error('could not save notes', error)
  }
}

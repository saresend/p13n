import { isDirty, snippetOf, sortNotes, titleOf, type Note } from '../notes.ts'

const formatDate = (iso: string) => {
  const date = new Date(iso)
  const today = new Date().toDateString() === date.toDateString()
  return today
    ? date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
    : date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
}

type Props = {
  notes: Note[]
  selectedId: string | null
  onCreate: () => void
  onSelect: (id: string) => void
}

export default function Sidebar({ notes, selectedId, onCreate, onSelect }: Props) {
  return (
    <aside className="sidebar">
      <header className="sidebar-head">
        <h1>notes</h1>
        <button className="icon-button" onClick={onCreate} title="New note" aria-label="New note">+</button>
      </header>
      <ul className="note-list">
        {sortNotes(notes).map((note) => (
          <li key={note.id}>
            <button
              className="note-item"
              aria-current={note.id === selectedId ? 'true' : undefined}
              onClick={() => onSelect(note.id)}
            >
              <span className="note-title">
                <span>{titleOf(note.text)}</span>
                {isDirty(note) && <span className="dot" title="Unsaved changes" />}
              </span>
              <span className="note-snippet">{snippetOf(note.text) || ' '}</span>
              <span className="note-date">{formatDate(note.updatedAt)}</span>
            </button>
          </li>
        ))}
      </ul>
      <footer className="sidebar-foot">{notes.length} {notes.length === 1 ? 'note' : 'notes'}</footer>
    </aside>
  )
}

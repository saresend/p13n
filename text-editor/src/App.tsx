import { useEffect } from 'react'
import Editor from './components/Editor.tsx'
import Sidebar from './components/Sidebar.tsx'
import { titleOf } from './notes.ts'
import { useNotes } from './useNotes.ts'

export default function App() {
  const { notes, selected, handlers } = useNotes()

  // ⌘S / Ctrl+S saves the open note instead of the page.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
        event.preventDefault()
        if (selected) handlers.save(selected.id)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selected, handlers])

  const confirmDelete = (id: string, text: string) => {
    if (window.confirm(`Delete “${titleOf(text)}”? This can't be undone.`)) handlers.remove(id)
  }

  return (
    <div className="app">
      <Sidebar notes={notes} selectedId={selected?.id ?? null} onCreate={handlers.create} onSelect={handlers.select} />
      {selected ? (
        <Editor
          key={selected.id}
          note={selected}
          onWrite={(text) => handlers.writeText(selected.id, text)}
          onSave={() => handlers.save(selected.id)}
          onDelete={() => confirmDelete(selected.id, selected.text)}
        />
      ) : (
        <main className="editor empty">
          <p>No note open.</p>
          <button className="text-button" onClick={handlers.create}>+ new note</button>
        </main>
      )}
    </div>
  )
}

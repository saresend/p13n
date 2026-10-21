import { useEffect } from 'react'
import Editor from './components/Editor.tsx'
import PersonalizationMenu from './components/PersonalizationMenu.tsx'
import Sidebar from './components/Sidebar.tsx'
import { titleOf } from './notes.ts'
import { useNotes } from './useNotes.ts'
import { usePersonalization } from './usePersonalization.ts'

export default function App() {
  const { notes, selected, handlers } = useNotes()
  const p13n = usePersonalization(handlers.apply)
  const { pending, applyPending, dismiss } = p13n

  // While a suggestion is showing, Tab applies it and Esc dismisses it (except inside the menu,
  // where Tab keeps moving focus).
  useEffect(() => {
    if (!pending) return
    const onKey = (event: KeyboardEvent) => {
      if (event.isComposing || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return
      if (event.target instanceof Element && event.target.closest('.p13n-panel')) return
      if (event.key === 'Tab') {
        event.preventDefault()
        applyPending()
      } else if (event.key === 'Escape') {
        dismiss()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [pending, applyPending, dismiss])

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
      <PersonalizationMenu
        url={p13n.url}
        status={p13n.status}
        error={p13n.error}
        pending={pending}
        onUrlChange={p13n.setUrl}
        onConnect={p13n.connect}
        onDisconnect={p13n.disconnect}
        onApply={applyPending}
      />
    </div>
  )
}

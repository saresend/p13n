import { useEffect, useMemo, useRef, useState } from 'react'
import { highlight } from '../markdown.ts'
import { isDirty, titleOf, wordCount, type Note } from '../notes.ts'

type Props = {
  note: Note
  onWrite: (text: string) => void
  onSave: () => void
  onDelete: () => void
}

export default function Editor({ note, onWrite, onSave, onDelete }: Props) {
  const html = useMemo(() => highlight(note.text), [note.text])
  const [focused, setFocused] = useState(false)
  const textarea = useRef<HTMLTextAreaElement>(null)

  // A new, empty note is ready to type into. (Editor is keyed by note id, so this runs per note.)
  const startsEmpty = useRef(!note.text)
  useEffect(() => {
    if (startsEmpty.current) textarea.current?.focus()
  }, [])

  const dirty = isDirty(note)
  return (
    <main className="editor">
      <div className="editor-scroll">
        <div className="editor-surface">
          <pre className="md-layer" aria-hidden="true" dangerouslySetInnerHTML={{ __html: html }} />
          <textarea
            ref={textarea}
            className="md-input"
            value={note.text}
            placeholder="# Title"
            spellCheck
            aria-label={titleOf(note.text)}
            onChange={(event) => onWrite(event.target.value)}
            onFocus={() => setFocused(true)}
            onBlur={() => setFocused(false)}
          />
        </div>
      </div>
      <footer className="pl status">
        <div className="segs">
          <span className={`seg mode ${focused ? 'insert' : ''}`}>{focused ? 'INSERT' : 'NORMAL'}</span>
          <span className="seg file"><span>{titleOf(note.text)}.md</span></span>
        </div>
        <span className="spacer" />
        <span className={`state ${dirty ? 'dirty' : ''}`}>{dirty ? '● modified' : 'saved'}</span>
        <span className="count">{wordCount(note.text)}w</span>
        <button className="pl-button" onClick={onSave} disabled={!dirty} title="Save (⌘S / Ctrl+S)">save</button>
        <button className="pl-button danger" onClick={onDelete} title="Delete note">delete</button>
      </footer>
    </main>
  )
}

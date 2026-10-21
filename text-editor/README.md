# text-editor

A minimal, Bear-style markdown notes app styled with the Noctis palette from the personal site.
Spec: Notion → Backlog → Personalization System for UX → Text-Editor.

```sh
npm ci
npm run dev      # http://localhost:5173
npm test         # reducer, storage, and highlighter tests (vitest)
npm run lint     # oxlint (needs Node 20.19+)
npm run build    # type-check and build to dist/
```

## Features

- Sidebar of notes, most recently edited first, with title, snippet, date, and an unsaved dot.
- `+` creates a note and focuses it.
- Live markdown editing: syntax stays visible but is styled (headings, bold, italic, code,
  fences, quotes, lists, tasks, links, rules).
- Saving: autosaves 0.8s after typing stops, on switching notes, on closing the tab, and with
  ⌘S / Ctrl+S or the `save` button. Notes are stored in `localStorage` for now.
- Delete from the status bar (asks first).

## Events

Every user action goes through a handler in `src/useNotes.ts`. The handler turns it into a
JSON event and the UI changes only by applying that event:

```json
{ "action": "write_text", "payload": { "id": "…", "text": "…" }, "at": "2026-10-20T12:00:00.000Z" }
```

| action       | payload        |
|--------------|----------------|
| `create`     | `{ id }`       |
| `select`     | `{ id }`       |
| `write_text` | `{ id, text }` |
| `save`       | `{ id, text }` |
| `delete`     | `{ id }`       |

`src/events.ts` defines the types and an `addSink` hook. To send events to a server, register a
sink in `src/main.tsx`:

```ts
addSink((event) => navigator.sendBeacon('/api/events', JSON.stringify(event)))
```

In dev, events are logged to the console. State is a pure function of the event stream
(`reduce` in `src/notes.ts`), so a server can rebuild it by replaying the log.

## Layout

- `src/events.ts`: event types, `emit`, `addSink`.
- `src/notes.ts`: note model and the pure `reduce`.
- `src/storage.ts`: `localStorage` persistence of saved text.
- `src/markdown.ts`: line-based markdown highlighter.
- `src/useNotes.ts`: state, handlers, autosave.
- `src/components/`: `Sidebar`, `Editor`.

The editor is a transparent `<textarea>` over a highlighted `<pre>` with the same text. Both use
one monospace font at one size, so headings are styled with color and weight rather than size.
`markdown.test.ts` checks that highlighting never changes the visible characters.

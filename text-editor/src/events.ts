// Every user action is a plain JSON event. The UI changes state only by emitting one,
// so the same stream can later be sent to a server (add a sink) or replayed through `reduce`.

export type UserAction =
  | { action: 'create'; payload: { id: string } }
  | { action: 'select'; payload: { id: string } }
  | { action: 'write_text'; payload: { id: string; text: string } }
  | { action: 'save'; payload: { id: string; text: string } }
  | { action: 'delete'; payload: { id: string } }

// `user` for things the person did; `system` for actions applied from a suggestion.
export type Source = 'user' | 'system'

export type UserEvent = UserAction & { at: string; source: Source }

export type Sink = (event: UserEvent) => void

const sinks = new Set<Sink>()

// Register a consumer for every event, e.g.
//   addSink((event) => navigator.sendBeacon('/api/events', JSON.stringify(event)))
// Returns a function that removes it.
export function addSink(sink: Sink): () => void {
  sinks.add(sink)
  return () => sinks.delete(sink)
}

// Stamp an action with the time and source and hand it to every sink. A failing sink never
// blocks the UI.
export function emit(action: UserAction, now = new Date(), source: Source = 'user'): UserEvent {
  const event = { ...action, at: now.toISOString(), source } as UserEvent
  for (const sink of sinks) {
    try {
      sink(event)
    } catch (error) {
      console.error('event sink failed', error)
    }
  }
  return event
}

export const newId = (): string =>
  globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`

// Client for the personalization server (../intelligence-engine). Connecting posts this app's
// vocabulary to /init, forwards every event to /action, and listens for suggestions over SSE.

import { addSink, newId, type UserAction, type UserEvent } from './events.ts'
import vocabulary from './vocabulary.json'

export type SuggestedAction = { action_name: string; payload: unknown; source: string }
export type Suggestion = { confidence: number; user_id: string; actions: SuggestedAction[] }
export type Status = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'error'

// One id per browser (so the server learns across visits) and one per page load.
const USER_KEY = 'p13n.text-editor.user-id'
export const userId = (() => {
  try {
    const existing = localStorage.getItem(USER_KEY)
    if (existing) return existing
    const id = newId()
    localStorage.setItem(USER_KEY, id)
    return id
  } catch {
    return newId()
  }
})()
export const sessionId = newId()

// The server's action shape.
export const toServerAction = (event: UserEvent) => ({
  user_id: userId,
  session_id: sessionId,
  action_name: event.action,
  payload: event.payload,
  source: event.source,
})

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null
const str = (value: unknown): value is string => typeof value === 'string'

// A suggested action as an editor action, or null if the editor can't apply it.
export function toUserAction({ action_name, payload }: SuggestedAction): UserAction | null {
  if (!isRecord(payload) || !str(payload.id)) return null
  const { id, text } = payload
  switch (action_name) {
    case 'create':
    case 'select':
    case 'delete':
      return { action: action_name, payload: { id } }
    case 'write_text':
    case 'save':
      return str(text) ? { action: action_name, payload: { id, text } } : null
    default:
      return null
  }
}

export function normalizeUrl(url: string): string {
  const trimmed = url.trim().replace(/\/+$/, '')
  return /^https?:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`
}

type Options = { onSuggestion: (suggestion: Suggestion) => void; onStatus: (status: Status, error?: string) => void }

// Returns a function that disconnects. Throws if /init fails.
export async function connect(url: string, { onSuggestion, onStatus }: Options): Promise<() => void> {
  const base = normalizeUrl(url)
  const res = await fetch(`${base}/init`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(vocabulary),
  })
  if (!res.ok) throw new Error(`init failed: ${res.status} ${await res.text().catch(() => '')}`.trim())

  const removeSink = addSink((event) => {
    fetch(`${base}/action`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(toServerAction(event)),
      keepalive: true,
    })
      .then(async (r) => {
        if (!r.ok) console.warn('[p13n] action rejected', r.status, await r.text())
      })
      .catch((error) => console.warn('[p13n] could not send action', error))
  })

  const source = new EventSource(`${base}/sse/suggestion?user_id=${encodeURIComponent(userId)}`)
  source.onopen = () => onStatus('connected')
  // EventSource retries on its own; CLOSED means it gave up.
  source.onerror = () =>
    onStatus(source.readyState === EventSource.CLOSED ? 'error' : 'reconnecting', 'lost connection to the server')
  source.addEventListener('suggestion', (message) => {
    try {
      onSuggestion(JSON.parse((message as MessageEvent<string>).data) as Suggestion)
    } catch (error) {
      console.warn('[p13n] bad suggestion', error)
    }
  })

  return () => {
    source.close()
    removeSink()
  }
}

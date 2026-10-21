import { useCallback, useEffect, useRef, useState } from 'react'
import type { UserAction } from './events.ts'
import { connect, type Status, type Suggestion, toUserAction } from './personalization.ts'

const URL_KEY = 'p13n.text-editor.server-url'
// 'off' after the user disconnects; otherwise the editor connects on load.
const AUTO_KEY = 'p13n.text-editor.auto-connect'
const DEFAULT_URL = 'http://localhost:8080'

export type Pending = { id: number; suggestion: Suggestion; actions: UserAction[] }

function read(key: string) {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value)
  } catch {
    // Remembering settings is a convenience only.
  }
}

const autoConnect = () => read(AUTO_KEY) !== 'off'

// Connection to the personalization server plus the latest suggestion, waiting to be applied.
export function usePersonalization(onApply: (actions: UserAction[]) => void) {
  const [url, setUrl] = useState(() => read(URL_KEY) ?? DEFAULT_URL)
  const [status, setStatus] = useState<Status>(() => (autoConnect() ? 'connecting' : 'idle'))
  const [error, setError] = useState<string | null>(null)
  const [pending, setPending] = useState<Pending | null>(null)
  const disconnectRef = useRef<(() => void) | null>(null)
  // Bumped by every connect/disconnect, so a connection that finishes opening after it was
  // superseded closes itself instead of running alongside the new one.
  const generation = useRef(0)
  const counter = useRef(0)

  const close = useCallback(() => {
    generation.current++
    disconnectRef.current?.()
    disconnectRef.current = null
  }, [])

  const connectTo = useCallback(async (target: string) => {
    close()
    const gen = generation.current
    const current = () => gen === generation.current
    setStatus('connecting')
    setError(null)
    setPending(null)
    write(URL_KEY, target)
    try {
      const disconnect = await connect(target, {
        onSuggestion: (suggestion) => {
          if (!current()) return
          const actions = suggestion.actions.map(toUserAction).filter((action) => action !== null)
          // Newer suggestions replace an unapplied one.
          if (actions.length) setPending({ id: ++counter.current, suggestion, actions })
        },
        onStatus: (next, message) => {
          if (!current()) return
          setStatus(next)
          setError(next === 'connected' ? null : (message ?? null))
        },
      })
      if (current()) disconnectRef.current = disconnect
      else disconnect()
    } catch (err) {
      if (!current()) return
      setStatus('error')
      const message = err instanceof Error ? err.message : String(err)
      setError(message === 'Failed to fetch' ? `can't reach ${target} (is it running, and does CORS allow this origin?)` : message)
    }
  }, [close])

  const disconnect = useCallback(() => {
    close()
    write(AUTO_KEY, 'off')
    setStatus('idle')
    setError(null)
    setPending(null)
  }, [close])

  // Connect on load unless the user turned it off; close on unmount. Deferred a tick so a
  // mount that's immediately undone (React StrictMode in dev) never starts a connection.
  useEffect(() => {
    const timer = autoConnect() ? setTimeout(() => void connectTo(read(URL_KEY) ?? DEFAULT_URL)) : undefined
    return () => {
      clearTimeout(timer)
      close()
    }
  }, [connectTo, close])

  const applyPending = useCallback(() => {
    if (!pending) return
    onApply(pending.actions)
    setPending(null)
  }, [pending, onApply])

  const dismiss = useCallback(() => setPending(null), [])

  return {
    url,
    setUrl,
    status,
    error,
    pending,
    connect: () => {
      write(AUTO_KEY, 'on')
      void connectTo(url)
    },
    disconnect,
    applyPending,
    dismiss,
  }
}

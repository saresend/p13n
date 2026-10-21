import { useState } from 'react'
import { userId, type Status } from '../personalization.ts'
import type { Pending } from '../usePersonalization.ts'

const STATUS_TEXT: Record<Status, string> = {
  idle: 'not connected',
  connecting: 'connecting…',
  connected: 'connected',
  reconnecting: 'reconnecting…',
  error: 'error',
}

type Props = {
  url: string
  status: Status
  error: string | null
  pending: Pending | null
  onUrlChange: (url: string) => void
  onConnect: () => void
  onDisconnect: () => void
  onApply: () => void
}

export default function PersonalizationMenu(props: Props) {
  const { url, status, error, pending } = props
  const [open, setOpen] = useState(false)
  const active = status !== 'idle'

  return (
    <div className="p13n">
      <button
        className={`p13n-pill ${pending ? 'suggesting' : ''}`}
        onClick={() => setOpen(!open)}
        aria-expanded={open}
        title={pending ? 'Suggestion ready: Tab to apply, Esc to dismiss' : 'Personalization'}
      >
        <span className={`p13n-dot ${status}`} />
        p13n
      </button>

      {open && (
        <form
          className="p13n-panel"
          onSubmit={(event) => {
            event.preventDefault()
            props.onConnect()
          }}
        >
          <label>
            <span>personalization server</span>
            <input
              value={url}
              onChange={(event) => props.onUrlChange(event.target.value)}
              placeholder="http://localhost:8080"
              spellCheck={false}
              autoComplete="off"
            />
          </label>
          <div className="p13n-row">
            <button type="submit" className="p13n-button primary" disabled={!url.trim() || status === 'connecting'}>
              {active ? 'reconnect' : 'connect'}
            </button>
            {active && <button type="button" className="p13n-button" onClick={props.onDisconnect}>disconnect</button>}
          </div>
          <p className={`p13n-status ${status}`}>{error ?? STATUS_TEXT[status]}</p>
          <p className="p13n-meta">user {userId.slice(0, 8)}</p>
        </form>
      )}

      {pending && (
        <button key={pending.id} className="p13n-cta" onClick={props.onApply}>
          tab to apply
        </button>
      )}
    </div>
  )
}

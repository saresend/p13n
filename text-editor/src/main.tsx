import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App.tsx'
import { addSink } from './events.ts'
import './index.css'
import './App.css'

// Until there's a server, log the event stream in dev so the instrumentation is visible.
if (import.meta.env.DEV) addSink((event) => console.debug('[event]', event))

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)

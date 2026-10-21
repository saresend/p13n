import hljs from 'highlight.js/lib/core'
import bash from 'highlight.js/lib/languages/bash'
import css from 'highlight.js/lib/languages/css'
import diff from 'highlight.js/lib/languages/diff'
import go from 'highlight.js/lib/languages/go'
import javascript from 'highlight.js/lib/languages/javascript'
import json from 'highlight.js/lib/languages/json'
import markdown from 'highlight.js/lib/languages/markdown'
import python from 'highlight.js/lib/languages/python'
import rust from 'highlight.js/lib/languages/rust'
import sql from 'highlight.js/lib/languages/sql'
import typescript from 'highlight.js/lib/languages/typescript'
import xml from 'highlight.js/lib/languages/xml'
import yaml from 'highlight.js/lib/languages/yaml'

// Only these languages are bundled; add more from highlight.js/lib/languages as needed.
// Aliases (ts, js, sh, py, rs, html, yml, …) come with each language.
const languages = { bash, css, diff, go, javascript, json, markdown, python, rust, sql, typescript, xml, yaml }
for (const [name, language] of Object.entries(languages)) hljs.registerLanguage(name, language)

// HTML for a fenced block's body, or null when the language is missing or unknown.
// highlight.js escapes the code and only wraps it in spans, so the characters are unchanged.
export function highlightCode(code: string, language: string): string | null {
  if (!language || !hljs.getLanguage(language)) return null
  return hljs.highlight(code, { language, ignoreIllegals: true }).value
}

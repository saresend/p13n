// Bear-style live markdown: syntax stays visible but gets styled. The output is drawn under a
// transparent textarea, so it must contain exactly the input's characters (only wrapped in
// spans); anything else would misalign the caret. markdown.test.ts checks that.

import { highlightCode } from './code.ts'

const escapeHtml = (text: string) =>
  text.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!)

const span = (cls: string, html: string) => `<span class="${cls}">${html}</span>`
const mark = (text: string) => span('md-mark', escapeHtml(text))

const INLINE = new RegExp(
  [
    String.raw`(\`+)([^\`\n]+?)\1`, // 1-2 code
    String.raw`(\*\*|__)(?=\S)([^\n]+?)(?<=\S)\3`, // 3-4 strong
    String.raw`(?<![\w*])(\*|_)(?=[^\s*_])([^\n]*?[^\s*_])\5(?![\w*])`, // 5-6 emphasis
    String.raw`(~~)(?=\S)([^\n]+?)(?<=\S)~~`, // 7-8 strikethrough
    String.raw`\[([^\]\n]*)\]\(([^)\s]*)\)`, // 9-10 link
  ].join('|'),
  'g',
)

export function inline(text: string): string {
  let html = ''
  let last = 0
  for (const m of text.matchAll(INLINE)) {
    html += escapeHtml(text.slice(last, m.index))
    last = m.index + m[0].length
    if (m[1]) html += span('md-code', mark(m[1]) + escapeHtml(m[2]) + mark(m[1]))
    else if (m[3]) html += span('md-strong', mark(m[3]) + inline(m[4]) + mark(m[3]))
    else if (m[5]) html += span('md-em', mark(m[5]) + inline(m[6]) + mark(m[5]))
    else if (m[7]) html += span('md-strike', mark('~~') + inline(m[8]) + mark('~~'))
    else html += span('md-link', mark('[') + span('md-link-text', inline(m[9])) + mark('](') + span('md-url', escapeHtml(m[10])) + mark(')'))
  }
  return html + escapeHtml(text.slice(last))
}

const FENCE = /^\s*(```|~~~)\s*([\w#+.-]*)/
const HEADING = /^(#{1,6})(\s+)(.*)$/
const RULE = /^\s*([-*_])(\s*\1){2,}\s*$/
const QUOTE = /^(\s*(?:>\s?)+)(.*)$/
const LIST = /^(\s*)([-*+]|\d+[.)])(\s+)(\[[ xX]\](?:\s|$))?(.*)$/

function block(line: string): string {
  let m: RegExpMatchArray | null
  if ((m = line.match(HEADING))) return span(`md-h md-h${m[1].length}`, mark(m[1]) + escapeHtml(m[2]) + inline(m[3]))
  if (RULE.test(line)) return span('md-hr', escapeHtml(line))
  if ((m = line.match(QUOTE))) return span('md-quote', mark(m[1]) + inline(m[2]))
  if ((m = line.match(LIST))) {
    const [, indent, bullet, gap, task, rest] = m
    const done = /\[[xX]\]/.test(task ?? '')
    const box = task ? span(`md-task${done ? ' md-done' : ''}`, escapeHtml(task)) : ''
    return escapeHtml(indent) + span('md-bullet', escapeHtml(bullet)) + escapeHtml(gap) + box + (done ? span('md-done', inline(rest)) : inline(rest))
  }
  return inline(line)
}

export function highlight(text: string): string {
  const lines = text.split('\n')
  const out: string[] = []
  for (let i = 0; i < lines.length; i++) {
    const open = lines[i].match(FENCE)
    if (!open) {
      out.push(block(lines[i]))
      continue
    }
    // A fenced block runs to the matching fence, or to the end while it's still being typed.
    out.push(span('md-fence', escapeHtml(lines[i])))
    let close = i + 1
    while (close < lines.length && lines[close].match(FENCE)?.[1] !== open[1]) close++
    const body = lines.slice(i + 1, close)
    if (body.length) {
      const code = body.join('\n')
      const html = highlightCode(code, open[2].toLowerCase())
      out.push(html === null ? span('md-code-block md-plain', escapeHtml(code)) : span('md-code-block', html))
    }
    if (close < lines.length) out.push(span('md-fence', escapeHtml(lines[close])))
    i = close
  }
  // A trailing newline needs a character after it, or the last empty line has no height.
  return out.join('\n') + (text.endsWith('\n') ? ' ' : '')
}

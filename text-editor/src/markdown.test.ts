import { describe, expect, it } from 'vitest'
import { highlight } from './markdown.ts'

// What the highlighted layer displays, which must match the textarea exactly.
const visibleText = (html: string) =>
  html.replace(/<[^>]+>/g, '').replace(/&(amp|lt|gt|quot|#x27);/g, (_, e: string) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#x27': "'" })[e]!)

const SAMPLE = `# Title with **bold** and \`code\`
## Second _level_

Some *emphasis*, __strong__, ~~gone~~, a [link](https://example.com) and snake_case_name.
Math: 2 * 3 * 4, a <b>tag</b> & "quotes".

> quoted **text**
>> nested

- item one
  * nested item
1. numbered
- [ ] todo
- [x] done

---

\`\`\`ts
const x = '<not html>' // **not bold**
\`\`\`
unclosed **bold and \`code
`

describe('highlight', () => {
  it('keeps exactly the input characters', () => {
    expect(visibleText(highlight(SAMPLE))).toBe(SAMPLE + ' ')
    for (const line of SAMPLE.split('\n')) expect(visibleText(highlight(line))).toBe(line)
  })

  it('styles common syntax', () => {
    const html = highlight(SAMPLE)
    expect(html).toContain('<span class="md-h md-h1"><span class="md-mark">#</span>')
    expect(html).toContain('<span class="md-strong"><span class="md-mark">**</span>bold')
    expect(html).toContain('<span class="md-em"><span class="md-mark">*</span>emphasis')
    expect(html).toContain('<span class="md-link-text">link</span>')
    expect(html).toContain('<span class="md-task md-done">[x] </span>')
    expect(html).toContain('<span class="md-hr">---</span>')
  })

  it('leaves fenced code and intraword underscores alone', () => {
    const html = highlight(SAMPLE)
    expect(html).toContain('<span class="hljs-string">&#x27;&lt;not html&gt;&#x27;</span>')
    expect(html).not.toContain('<span class="md-strong"><span class="md-mark">**</span>not bold')
    expect(html).toContain('snake_case_name')
    expect(html).toContain('2 * 3 * 4')
  })

  it('highlights fenced code by language, as one block', () => {
    const code = '```ts src/a.ts\n/* spans\n   lines */\nconst n: number = 1 // "<b>"\n```'
    const html = highlight(code)
    expect(visibleText(html)).toBe(code)
    expect(html).toContain('<span class="hljs-keyword">const</span>')
    expect(html).toContain('<span class="hljs-comment">/* spans\n   lines */</span>')
    expect(html).toContain('<span class="hljs-number">1</span>')
  })

  it('keeps unknown or missing languages plain, and unclosed fences run to the end', () => {
    expect(highlight('```nope\nconst x\n```')).toContain('<span class="md-code-block md-plain">const x</span>')
    const open = highlight('```py\ndef f():\n  # **not bold**')
    expect(open).toContain('<span class="hljs-keyword">def</span>')
    expect(open).not.toContain('md-strong')
  })

  it('escapes HTML', () => {
    expect(highlight('<script>alert(1)</script>')).toBe('&lt;script&gt;alert(1)&lt;/script&gt;')
  })
})

//! Markdown syntax highlighting for the editor.
//!
//! The editor is a plain `<textarea>` with a styled copy of its text drawn
//! underneath. For the two to line up, every character of the source must
//! appear exactly once, in order, in the highlighted output, and styling may
//! only change things that don't affect glyph widths (color, background,
//! underline, synthesized italic, text-shadow).

use yew::prelude::*;

/// A run of text with an optional CSS class.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub class: Option<&'static str>,
    pub text: String,
}

/// One source line (without its `\n`), with an optional class for the whole line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub class: Option<&'static str>,
    pub spans: Vec<Span>,
}

fn span(class: Option<&'static str>, text: &str) -> Span {
    Span { class, text: text.to_string() }
}

const MARK: Option<&str> = Some("md-mark");

/// Split `text` into highlighted lines. Joining every span's text, with `\n`
/// between lines, gives back `text` exactly.
pub fn highlight(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut in_fence = false;
    for raw in text.split('\n') {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            lines.push(Line { class: Some("md-code-block"), spans: vec![span(MARK, raw)] });
            continue;
        }
        if in_fence {
            lines.push(Line { class: Some("md-code-block"), spans: vec![span(None, raw)] });
            continue;
        }
        lines.push(block_line(raw));
    }
    lines
}

fn block_line(raw: &str) -> Line {
    let indent_len = raw.len() - raw.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = raw.split_at(indent_len);

    // Heading: up to six #'s and a space (or nothing else).
    let hashes = rest.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && (rest.len() == hashes || rest.as_bytes()[hashes] == b' ') && indent.len() < 4 {
        let marker_len = (hashes + 1).min(rest.len());
        let mut spans = vec![span(None, indent), span(MARK, &rest[..marker_len])];
        spans.extend(inline(&rest[marker_len..]));
        let class = if hashes == 1 { "md-h md-h1" } else { "md-h" };
        return Line { class: Some(class), spans };
    }

    // Horizontal rule.
    let compact: String = rest.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() >= 3 && ["-", "*", "_"].iter().any(|c| compact.chars().all(|x| x.to_string() == *c)) {
        return Line { class: None, spans: vec![span(None, indent), span(MARK, rest)] };
    }

    // Blockquote: one or more `>` (each optionally followed by a space).
    if rest.starts_with('>') {
        let mut n = 0;
        let b = rest.as_bytes();
        while n < b.len() && b[n] == b'>' {
            n += 1;
            if n < b.len() && b[n] == b' ' {
                n += 1;
            }
        }
        let mut spans = vec![span(None, indent), span(MARK, &rest[..n])];
        spans.extend(inline(&rest[n..]));
        return Line { class: Some("md-quote"), spans };
    }

    // List item: `- `, `* `, `+ `, `1. ` or `1) `, plus an optional task box.
    if let Some(marker_len) = list_marker(rest) {
        let mut marker_len = marker_len;
        for task in ["[ ] ", "[x] ", "[X] "] {
            if rest[marker_len..].starts_with(task) {
                marker_len += task.len();
            }
        }
        let mut spans = vec![span(None, indent), span(Some("md-bullet"), &rest[..marker_len])];
        spans.extend(inline(&rest[marker_len..]));
        return Line { class: None, spans };
    }

    let mut spans = vec![span(None, indent)];
    spans.extend(inline(rest));
    Line { class: None, spans }
}

fn list_marker(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && b[1] == b' ' {
        return Some(2);
    }
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&digits) && b.len() > digits + 1 && matches!(b[digits], b'.' | b')') && b[digits + 1] == b' ' {
        return Some(digits + 2);
    }
    None
}

/// Find the closing `delim` for an opening at `open`, on the same line, with
/// at least one character between and no whitespace just inside the closer.
fn closing(s: &str, open: usize, delim: &str) -> Option<usize> {
    let from = open + delim.len();
    let rel = s[from..].find(delim)?;
    let close = from + rel;
    if close == from || s[..close].ends_with(char::is_whitespace) {
        return None;
    }
    Some(close)
}

/// Inline markup: `code`, **strong**, *em*, _em_, [link](url).
fn inline(s: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain_start = 0;
    let mut i = 0;
    let bytes = s.as_bytes();

    let flush = |out: &mut Vec<Span>, from: usize, to: usize| {
        if to > from {
            out.push(span(None, &s[from..to]));
        }
    };

    while i < bytes.len() {
        let c = bytes[i];
        let prev_alnum = i > 0 && (bytes[i - 1] as char).is_alphanumeric();
        let next_space = bytes.get(i + 1).is_none_or(|b| b.is_ascii_whitespace());

        // `code`
        if c == b'`' {
            if let Some(close) = s[i + 1..].find('`').map(|r| i + 1 + r).filter(|&e| e > i + 1) {
                flush(&mut out, plain_start, i);
                out.push(span(MARK, "`"));
                out.push(span(Some("md-code"), &s[i + 1..close]));
                out.push(span(MARK, "`"));
                i = close + 1;
                plain_start = i;
                continue;
            }
        }

        // **strong** / __strong__
        if (s[i..].starts_with("**") || (s[i..].starts_with("__") && !prev_alnum))
            && !bytes.get(i + 2).is_none_or(|b| b.is_ascii_whitespace())
        {
            let delim = &s[i..i + 2];
            if let Some(close) = closing(s, i, delim) {
                flush(&mut out, plain_start, i);
                out.push(span(MARK, delim));
                out.extend(
                    inline(&s[i + 2..close])
                        .into_iter()
                        .map(|sp| Span { class: Some(sp.class.map_or("md-strong", strong_with)), ..sp }),
                );
                out.push(span(MARK, delim));
                i = close + 2;
                plain_start = i;
                continue;
            }
        }

        // *em* / _em_
        if (c == b'*' || (c == b'_' && !prev_alnum)) && !next_space {
            let delim = if c == b'*' { "*" } else { "_" };
            if let Some(close) = closing(s, i, delim) {
                let after_alnum = bytes.get(close + 1).is_some_and(|b| (*b as char).is_alphanumeric());
                let doubled = s[close..].starts_with(if c == b'*' { "**" } else { "__" });
                if !doubled && !(c == b'_' && after_alnum) {
                    flush(&mut out, plain_start, i);
                    out.push(span(MARK, delim));
                    out.push(span(Some("md-em"), &s[i + 1..close]));
                    out.push(span(MARK, delim));
                    i = close + 1;
                    plain_start = i;
                    continue;
                }
            }
        }

        // [text](url)
        if c == b'[' {
            if let Some(mid) = s[i..].find("](").map(|r| i + r) {
                if let Some(end) = s[mid + 2..].find(')').map(|r| mid + 2 + r) {
                    if !s[i + 1..mid].contains(']') {
                        flush(&mut out, plain_start, i);
                        out.push(span(MARK, "["));
                        out.push(span(Some("md-link"), &s[i + 1..mid]));
                        out.push(span(Some("md-url"), &s[mid..=end]));
                        i = end + 1;
                        plain_start = i;
                        continue;
                    }
                }
            }
        }

        i += s[i..].chars().next().map_or(1, char::len_utf8);
    }
    flush(&mut out, plain_start, bytes.len());
    out
}

/// Class for a span nested inside **strong**.
fn strong_with(class: &'static str) -> &'static str {
    match class {
        "md-em" => "md-strong md-em",
        "md-mark" => "md-mark",
        other => other,
    }
}

/// The URL of the link (if any) covering byte offset `at` in `text`.
pub fn link_at(text: &str, at: usize) -> Option<String> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let line = &text[line_start..line_end];
    let col = at - line_start;
    let mut pos = 0;
    let mut link_text_range = None;
    for sp in &block_line(line).spans {
        let end = pos + sp.text.len();
        match sp.class {
            Some("md-link") => link_text_range = Some(pos..end),
            Some("md-url") => {
                // Covers `[text](url)` from the `[` to the `)`.
                let start = link_text_range.take().map_or(pos, |r| r.start.saturating_sub(1));
                if (start..=end).contains(&col) {
                    let url = sp.text.trim_start_matches("](").trim_end_matches(')').trim();
                    return (!url.is_empty()).then(|| url.to_string());
                }
            }
            _ => {}
        }
        pos = end;
    }
    None
}

/// Render highlighted text for the layer under the textarea.
pub fn render(text: &str) -> Html {
    let lines = highlight(text);
    let last = lines.len().saturating_sub(1);
    html! {
        <>
            { for lines.into_iter().enumerate().map(|(i, line)| html! {
                <>
                    <span class={classes!(line.class)}>
                        { for line.spans.into_iter().map(|sp| match sp.class {
                            Some(c) => html! { <span class={c}>{ sp.text }</span> },
                            None => html! { { sp.text } },
                        }) }
                    </span>
                    { if i < last { "\n" } else { "" } }
                </>
            }) }
            // A trailing newline in a textarea still takes up a line.
            { "\u{200b}" }
        </>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn classes_of(s: &str) -> Vec<(Option<&'static str>, String)> {
        highlight(s)[0].spans.iter().filter(|sp| !sp.text.is_empty()).map(|sp| (sp.class, sp.text.clone())).collect()
    }

    #[test]
    fn preserves_text_exactly() {
        let samples = [
            "",
            "\n",
            "plain",
            "# Title\n\nSome **bold** and *em* and `code` and [a link](https://x.y).\n",
            "> quote with _em_\n>> nested\n- item\n  * sub\n10. ten\n- [ ] task\n---\n",
            "```rust\nlet x = a * b * c;\n```\nafter",
            "snake_case_name and 2*3*4 and ** not bold ** and [broken](link and `unclosed",
            "émoji 🎉 **gras ünïcödé** _ita_ `ç`",
            "#hashtag not heading\n####### seven",
            "**bold *nested em* bold**",
        ];
        for s in samples {
            assert_eq!(flatten(&highlight(s)), s, "round trip of {s:?}");
        }
    }

    #[test]
    fn recognizes_blocks() {
        assert_eq!(highlight("# T")[0].class, Some("md-h md-h1"));
        assert_eq!(highlight("### T")[0].class, Some("md-h"));
        assert_eq!(highlight("#tag")[0].class, None);
        assert_eq!(highlight("> q")[0].class, Some("md-quote"));
        assert_eq!(highlight("```\n# not a heading\n```")[1].class, Some("md-code-block"));
        assert_eq!(classes_of("- item")[0], (Some("md-bullet"), "- ".into()));
        assert_eq!(classes_of("2) item")[0], (Some("md-bullet"), "2) ".into()));
    }

    #[test]
    fn recognizes_inline() {
        assert!(classes_of("a **b** c").contains(&(Some("md-strong"), "b".into())));
        assert!(classes_of("a *b* c").contains(&(Some("md-em"), "b".into())));
        assert!(classes_of("a _b_ c").contains(&(Some("md-em"), "b".into())));
        assert!(classes_of("a `b*c*` d").contains(&(Some("md-code"), "b*c*".into())));
        assert!(classes_of("[t](u)").contains(&(Some("md-link"), "t".into())));
        assert!(classes_of("**a *b* c**").contains(&(Some("md-strong md-em"), "b".into())));
        // Not markup:
        assert!(classes_of("snake_case_name").iter().all(|(c, _)| c.is_none()));
        assert!(classes_of("2 * 3 * 4").iter().all(|(c, _)| c.is_none()));
    }

    #[test]
    fn finds_links_under_cursor() {
        let t = "see [docs](https://example.com) here\nnext";
        let at = t.find("docs").unwrap() + 1;
        assert_eq!(link_at(t, at).as_deref(), Some("https://example.com"));
        assert_eq!(link_at(t, t.find("https").unwrap()).as_deref(), Some("https://example.com"));
        assert_eq!(link_at(t, 1), None);
        assert_eq!(link_at(t, t.len()), None);
    }
}

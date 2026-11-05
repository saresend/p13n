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
    // Inside a fenced block: the fence's language tag, lowercased.
    let mut fence: Option<String> = None;
    for raw in text.split('\n') {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let mut spans = vec![span(MARK, raw)];
            if fence.is_some() {
                fence = None;
            } else {
                let tag = trimmed[3..].trim();
                fence = Some(tag.split_whitespace().next().unwrap_or("").to_lowercase());
                if !tag.is_empty() {
                    let split = raw.len() - trimmed.len() + 3;
                    let (marks, rest) = raw.split_at(split);
                    spans = vec![span(MARK, marks), span(Some("md-lang"), rest)];
                }
            }
            lines.push(Line { class: Some("md-code-block"), spans });
            continue;
        }
        if let Some(lang) = &fence {
            lines.push(Line { class: Some("md-code-block"), spans: code::spans(raw, lang) });
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

/// Syntax highlighting for fenced code blocks: keywords, strings, comments,
/// numbers, types and calls, line by line.
mod code {
    use super::{Span, span};

    const RUST: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self",
        "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
    ];
    const PYTHON: &[&str] = &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except",
        "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None", "nonlocal", "not",
        "or", "pass", "raise", "return", "self", "True", "try", "while", "with", "yield",
    ];
    const JS: &[&str] = &[
        "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do", "else",
        "export", "extends", "false", "finally", "for", "from", "function", "if", "import", "in", "instanceof",
        "interface", "let", "new", "null", "of", "return", "switch", "this", "throw", "true", "try", "type",
        "typeof", "undefined", "var", "void", "while", "yield",
    ];
    const GO: &[&str] = &[
        "break", "case", "chan", "const", "continue", "default", "defer", "else", "false", "for", "func", "go", "if",
        "import", "interface", "map", "nil", "package", "range", "return", "select", "struct", "switch", "true",
        "type", "var",
    ];
    const C: &[&str] = &[
        "auto", "bool", "break", "case", "catch", "char", "class", "const", "continue", "default", "delete", "do",
        "double", "else", "enum", "extends", "false", "final", "float", "for", "if", "import", "int", "long", "new",
        "null", "nullptr", "package", "private", "protected", "public", "return", "short", "static", "struct",
        "switch", "this", "throw", "true", "try", "typedef", "unsigned", "void", "while",
    ];
    const SHELL: &[&str] = &[
        "case", "do", "done", "echo", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in", "local",
        "return", "then", "while",
    ];
    const SQL: &[&str] = &[
        "and", "as", "by", "create", "delete", "desc", "from", "group", "insert", "into", "join", "limit", "not",
        "null", "on", "or", "order", "select", "set", "table", "update", "values", "where",
    ];
    const DATA: &[&str] = &["true", "false", "null", "yes", "no"];

    struct Lang {
        keywords: &'static [&'static str],
        line_comments: &'static [&'static str],
        case_insensitive: bool,
    }

    fn lang(tag: &str) -> Lang {
        let (keywords, line_comments): (&[&str], &[&str]) = match tag {
            "rust" | "rs" => (RUST, &["//"]),
            "python" | "py" => (PYTHON, &["#"]),
            "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" => (JS, &["//"]),
            "go" => (GO, &["//"]),
            "c" | "cpp" | "c++" | "h" | "java" | "cs" | "csharp" | "kotlin" | "swift" => (C, &["//"]),
            "sh" | "bash" | "zsh" | "shell" | "fish" | "console" => (SHELL, &["#"]),
            "sql" => (SQL, &["--"]),
            "json" | "yaml" | "yml" | "toml" => (DATA, &["#"]),
            // Unknown or missing tag: a little of everything.
            _ => (&[], &["//", "#"]),
        };
        Lang { keywords, line_comments, case_insensitive: tag == "sql" }
    }

    fn is_keyword(l: &Lang, word: &str, tag: &str) -> bool {
        if l.keywords.is_empty() {
            let common = [RUST, PYTHON, JS, GO];
            return common.iter().any(|set| set.contains(&word)) && tag.is_empty();
        }
        if l.case_insensitive {
            l.keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
        } else {
            l.keywords.contains(&word)
        }
    }

    pub fn spans(line: &str, tag: &str) -> Vec<Span> {
        let l = lang(tag);
        let b = line.as_bytes();
        let mut out = Vec::new();
        let mut plain = 0;
        let mut i = 0;
        let push = |out: &mut Vec<Span>, plain: &mut usize, from: usize, to: usize, class: &'static str| {
            if from > *plain {
                out.push(span(None, &line[*plain..from]));
            }
            out.push(span(Some(class), &line[from..to]));
            *plain = to;
        };
        while i < b.len() {
            let c = b[i];
            // Line comment: the rest of the line. `#` only counts at the start
            // of a word, so `a#b` or `#[derive]`-style text stays code.
            if let Some(m) = l.line_comments.iter().find(|m| line[i..].starts_with(**m)) {
                let starts_word = i == 0 || !(b[i - 1] as char).is_alphanumeric();
                let rust_attr = *m == "#" && b.get(i + 1) == Some(&b'[');
                if starts_word && !rust_attr {
                    push(&mut out, &mut plain, i, b.len(), "tok-comment");
                    i = b.len();
                    continue;
                }
            }
            // Block comment opened and closed on this line.
            if line[i..].starts_with("/*") {
                let end = line[i + 2..].find("*/").map_or(b.len(), |r| i + 2 + r + 2);
                push(&mut out, &mut plain, i, end, "tok-comment");
                i = end;
                continue;
            }
            // Strings. A single quote in Rust is usually a lifetime, so only
            // treat short ones ('a', '\n') as character literals there.
            if c == b'"' || c == b'`' || c == b'\'' {
                let mut j = i + 1;
                while j < b.len() && b[j] != c {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                let closed = j < b.len();
                let rust_lifetime = c == b'\'' && matches!(tag, "rust" | "rs") && j - i > 4;
                if closed && !rust_lifetime {
                    push(&mut out, &mut plain, i, j + 1, "tok-str");
                    i = j + 1;
                    continue;
                }
            }
            let prev_ident = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
            // Numbers.
            if c.is_ascii_digit() && !prev_ident {
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'.') {
                    j += 1;
                }
                push(&mut out, &mut plain, i, j, "tok-num");
                i = j;
                continue;
            }
            // Words: keywords, Types, calls().
            if (c.is_ascii_alphabetic() || c == b'_') && !prev_ident {
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let word = &line[i..j];
                let class = if is_keyword(&l, word, tag) {
                    Some("tok-kw")
                } else if c.is_ascii_uppercase() && word.len() > 1 {
                    Some("tok-type")
                } else if b.get(j) == Some(&b'(') || (b.get(j) == Some(&b'!') && b.get(j + 1) == Some(&b'(')) {
                    Some("tok-fn")
                } else {
                    None
                };
                if let Some(class) = class {
                    push(&mut out, &mut plain, i, j, class);
                }
                i = j;
                continue;
            }
            i += line[i..].chars().next().map_or(1, char::len_utf8);
        }
        if plain < b.len() {
            out.push(span(None, &line[plain..]));
        }
        if out.is_empty() {
            out.push(span(None, ""));
        }
        out
    }
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
    fn highlights_code_blocks() {
        let t = "```rust\nfn main() { let s = \"hi\"; // note\n```\nafter";
        assert_eq!(flatten(&highlight(t)), t);
        let lines = highlight(t);
        let toks: Vec<(Option<&str>, &str)> = lines[1].spans.iter().map(|s| (s.class, s.text.as_str())).collect();
        assert!(toks.contains(&(Some("tok-kw"), "fn")));
        assert!(toks.contains(&(Some("tok-fn"), "main")));
        assert!(toks.contains(&(Some("tok-kw"), "let")));
        assert!(toks.contains(&(Some("tok-str"), "\"hi\"")));
        assert!(toks.contains(&(Some("tok-comment"), "// note")));
        assert_eq!(lines[0].spans[1].class, Some("md-lang"));
        assert_eq!(lines[3].class, None, "text after the fence is not code");

        let py = "```py\ndef f(x=1):  # c\n    return 'a' + str(2.5)\n```";
        assert_eq!(flatten(&highlight(py)), py);
        let line2: Vec<_> = highlight(py)[2].spans.iter().map(|s| (s.class, s.text.clone())).collect();
        assert!(line2.contains(&(Some("tok-str"), "'a'".into())));
        assert!(line2.contains(&(Some("tok-num"), "2.5".into())));

        // A Rust lifetime is not a string.
        let lt = "```rust\nfn f<'a>(x: &'a str) {}\n```";
        assert!(highlight(lt)[1].spans.iter().all(|s| s.class != Some("tok-str")));
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

//! Pure functions that decide what a piece of copied text is.
//!
//! Everything here runs on every copy, so each function looks at a bounded
//! prefix of the input and never allocates proportionally to a large clip.

use crate::model::ClipKind;

/// Longest input (after trimming) that can still be a CSS color.
const MAX_COLOR_LEN: usize = 64;
/// Bytes of a clip inspected by the code heuristic.
const CODE_SAMPLE_BYTES: usize = 4096;
/// Non-blank lines inspected by the code heuristic.
const CODE_SAMPLE_LINES: usize = 64;

/// Classify text-like clipboard content as `Text`, `Link`, `Code` or `Color`.
/// Never returns `Image` or `File`; the watcher sets those from the payload
/// type. Must be cheap: it runs on every copy.
pub fn classify_text(text: &str) -> ClipKind {
    let t = text.trim();
    if t.is_empty() {
        return ClipKind::Text;
    }
    if parse_color(t).is_some() {
        return ClipKind::Color;
    }
    if is_link(t) {
        return ClipKind::Link;
    }
    if looks_like_code(t) {
        return ClipKind::Code;
    }
    ClipKind::Text
}

/// Parse a CSS-style color (`#RGB`, `#RRGGBB`, `#RRGGBBAA`, `rgb(...)`,
/// `rgba(...)`, `hsl(...)`) and return it normalized as uppercase `#RRGGBB`
/// or `#RRGGBBAA`. Returns `None` when the whole trimmed input is not a color.
///
/// Also accepts `#RGBA` and `hsla(...)`, and both the legacy comma syntax and
/// the modern space syntax (`rgb(0 0 0 / 50%)`). Out-of-range components are
/// rejected rather than clamped. An explicit alpha always yields `#RRGGBBAA`.
pub fn parse_color(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.len() > MAX_COLOR_LEN {
        return None;
    }
    if let Some(hex) = t.strip_prefix('#') {
        return parse_hex(hex);
    }
    let open = t.find('(')?;
    let inner = t[open + 1..].strip_suffix(')')?;
    let name = t[..open].trim_end().to_ascii_lowercase();
    let (comps, alpha) = split_components(inner)?;
    let rgb = match name.as_str() {
        "rgb" | "rgba" => parse_rgb(&comps)?,
        "hsl" | "hsla" => parse_hsl(&comps)?,
        _ => return None,
    };
    let alpha = match alpha {
        Some(a) => Some(parse_alpha(a)?),
        None => None,
    };
    Some(format_hex(rgb, alpha))
}

/// Single-pass preview: trims, collapses runs of blank lines, and truncates
/// to at most `max_chars` characters on a char boundary.
///
/// A run of blank (or whitespace-only) lines becomes one empty line, `\r\n`
/// becomes `\n`, and indentation inside the text is kept. Stops reading as
/// soon as `max_chars` characters are produced.
pub fn preview(text: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars.min(text.len()));
    let mut count = 0usize;
    let mut started = false;
    let mut pending_blank = false;
    'lines: for raw in text.split('\n') {
        if count >= max_chars {
            break;
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim().is_empty() {
            pending_blank = started;
            continue;
        }
        let line = if started { line } else { line.trim_start() };
        if started {
            let breaks = if pending_blank { 2 } else { 1 };
            for _ in 0..breaks {
                if count >= max_chars {
                    break 'lines;
                }
                out.push('\n');
                count += 1;
            }
        }
        started = true;
        pending_blank = false;
        for ch in line.chars() {
            if count >= max_chars {
                break 'lines;
            }
            out.push(ch);
            count += 1;
        }
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out
}

// ---------------------------------------------------------------- colors

fn parse_hex(hex: &str) -> Option<String> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let up = hex.to_ascii_uppercase();
    match up.len() {
        3 | 4 => {
            let mut s = String::with_capacity(9);
            s.push('#');
            for c in up.chars() {
                s.push(c);
                s.push(c);
            }
            Some(s)
        }
        6 | 8 => Some(format!("#{up}")),
        _ => None,
    }
}

/// Splits the inside of `rgb(...)` into three components and an optional
/// alpha. Accepts `a, b, c[, d]` or `a b c[ / d]`.
fn split_components(inner: &str) -> Option<(Vec<&str>, Option<&str>)> {
    let inner = inner.trim();
    let mut comps: Vec<&str>;
    let alpha;
    if inner.contains(',') {
        if inner.contains('/') {
            return None;
        }
        comps = inner.split(',').map(str::trim).collect();
        if comps.iter().any(|c| c.is_empty()) {
            return None;
        }
        alpha = if comps.len() == 4 { comps.pop() } else { None };
    } else {
        let mut halves = inner.splitn(2, '/');
        comps = halves.next()?.split_whitespace().collect();
        alpha = match halves.next() {
            Some(a) => {
                let a = a.trim();
                if a.is_empty() || a.contains(char::is_whitespace) {
                    return None;
                }
                Some(a)
            }
            None => None,
        };
    }
    if comps.len() != 3 {
        return None;
    }
    Some((comps, alpha))
}

/// Strict decimal: `[+-]?(digits[.digits]|.digits)`. Rejects `inf`, `NaN`,
/// exponents and empty input, which `str::parse::<f64>` would accept.
fn number(s: &str) -> Option<f64> {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    let ok = match frac {
        Some(f) => digits(int) && digits(f) && !f.is_empty(),
        None => digits(int) && !int.is_empty(),
    };
    if !ok {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn percent(s: &str) -> Option<f64> {
    number(s.strip_suffix('%')?)
}

fn parse_rgb(comps: &[&str]) -> Option<[f64; 3]> {
    let mut rgb = [0.0; 3];
    for (slot, c) in rgb.iter_mut().zip(comps) {
        let v = if c.ends_with('%') {
            let p = percent(c)?;
            if !(0.0..=100.0).contains(&p) {
                return None;
            }
            p / 100.0 * 255.0
        } else {
            let n = number(c)?;
            if !(0.0..=255.0).contains(&n) {
                return None;
            }
            n
        };
        *slot = v;
    }
    Some(rgb)
}

fn parse_hsl(comps: &[&str]) -> Option<[f64; 3]> {
    let hue = number(comps[0].strip_suffix("deg").unwrap_or(comps[0]))?;
    let sat = percent(comps[1])?;
    let light = percent(comps[2])?;
    if !(0.0..=100.0).contains(&sat) || !(0.0..=100.0).contains(&light) {
        return None;
    }
    let (s, l) = (sat / 100.0, light / 100.0);
    let h = hue.rem_euclid(360.0) / 60.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Some([(r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0])
}

fn parse_alpha(a: &str) -> Option<f64> {
    let v = if a.ends_with('%') {
        percent(a)? / 100.0
    } else {
        number(a)?
    };
    (0.0..=1.0).contains(&v).then_some(v * 255.0)
}

fn format_hex(rgb: [f64; 3], alpha: Option<f64>) -> String {
    let byte = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    let [r, g, b] = rgb.map(byte);
    match alpha {
        Some(a) => format!("#{r:02X}{g:02X}{b:02X}{:02X}", byte(a)),
        None => format!("#{r:02X}{g:02X}{b:02X}"),
    }
}

// ----------------------------------------------------------------- links

fn starts_with_ignore_case(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// A single URL with a web, FTP or mail scheme, or a `www.` host. Bare
/// domains (`example.com`) are text: too many false positives.
fn is_link(t: &str) -> bool {
    if t.contains(char::is_whitespace) {
        return false;
    }
    for scheme in ["https://", "http://", "ftp://", "ftps://"] {
        if starts_with_ignore_case(t, scheme) {
            let rest = &t[scheme.len()..];
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            return host.bytes().any(|b| b.is_ascii_alphanumeric());
        }
    }
    if starts_with_ignore_case(t, "mailto:") {
        let addr = &t["mailto:".len()..];
        return matches!(addr.split_once('@'), Some((u, d)) if !u.is_empty() && !d.is_empty());
    }
    if starts_with_ignore_case(t, "www.") {
        let host = t["www.".len()..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        return host.contains('.')
            && !host.ends_with('.')
            && !host.starts_with('.')
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':');
    }
    false
}

// ------------------------------------------------------------------ code

/// Line openers that only count when other syntax is present on the line.
const KEYWORDS: &[&str] = &[
    "fn ",
    "pub ",
    "def ",
    "class ",
    "function ",
    "func ",
    "const ",
    "let ",
    "var ",
    "import ",
    "export ",
    "from ",
    "return ",
    "use ",
    "struct ",
    "impl ",
    "enum ",
    "interface ",
    "package ",
    "public ",
    "private ",
    "protected ",
    "static ",
    "async ",
    "await ",
    "else",
    "elif ",
    "try:",
    "except",
    "if (",
    "if ",
    "for (",
    "for ",
    "while (",
    "while ",
    "switch (",
    "match ",
    "type ",
    "module ",
    "#define ",
    "@",
];

/// Line openers that are code on their own.
const STRONG: &[&str] = &[
    "#include",
    "#import",
    "#!/",
    "<?php",
    "<?xml",
    "#pragma",
    "SELECT ",
    "INSERT INTO ",
    "UPDATE ",
    "DELETE FROM ",
    "CREATE TABLE ",
    "CREATE INDEX ",
    "ALTER TABLE ",
    "DROP TABLE ",
    "WITH ",
];

const OPERATORS: &[&str] = &[
    "=>", "->", "::", "==", "!=", "&&", "||", "+=", "-=", "</", "/>", "()", "{}", "[]", ":=", "){",
    ") {",
];

fn sample(t: &str) -> &str {
    if t.len() <= CODE_SAMPLE_BYTES {
        return t;
    }
    let mut end = CODE_SAMPLE_BYTES;
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    &t[..end]
}

fn line_score(line: &str) -> u32 {
    let body = line.trim();
    if matches!(
        body,
        "}" | "};" | "})" | "});" | ")" | ");" | "]" | "];" | "end"
    ) {
        return 2;
    }
    if STRONG.iter().any(|k| body.starts_with(k)) {
        return 2;
    }
    let mut score = 0;
    let keyword = KEYWORDS.iter().any(|k| body.starts_with(k));
    if keyword {
        score += 1;
    }
    if body.ends_with(['{', ';', '}']) || (keyword && body.ends_with(':')) {
        score += 1;
    }
    if OPERATORS.iter().any(|op| body.contains(op)) {
        score += 1;
    }
    if line.starts_with("  ") || line.starts_with('\t') {
        score += 1;
    }
    score
}

fn looks_like_code(t: &str) -> bool {
    let s = sample(t);
    let (first, last) = (s.as_bytes()[0], t.as_bytes()[t.len() - 1]);
    if matches!(first, b'{' | b'[') && matches!(last, b'}' | b']') && s.contains("\":") {
        return true; // JSON
    }
    if first == b'<' && last == b'>' && (s.contains("</") || s.contains("/>")) {
        return true; // HTML / XML
    }
    let mut lines = 0u32;
    let mut codey = 0u32;
    for line in s
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(CODE_SAMPLE_LINES)
    {
        lines += 1;
        if line_score(line) >= 2 {
            codey += 1;
        }
    }
    codey > 0 && codey * 2 >= lines
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_color ----

    #[test]
    fn hex_colors_normalize_to_uppercase() {
        assert_eq!(parse_color("#ff0000").as_deref(), Some("#FF0000"));
        assert_eq!(parse_color("  #AbC  ").as_deref(), Some("#AABBCC"));
        assert_eq!(parse_color("#abcd").as_deref(), Some("#AABBCCDD"));
        assert_eq!(parse_color("#11223344").as_deref(), Some("#11223344"));
    }

    #[test]
    fn invalid_hex_is_not_a_color() {
        for s in [
            "#",
            "#12",
            "#12345",
            "#1234567",
            "#GGGGGG",
            "ff0000",
            "#ff0000 x",
            "##ff0000",
        ] {
            assert_eq!(parse_color(s), None, "{s:?}");
        }
    }

    #[test]
    fn rgb_functions() {
        assert_eq!(parse_color("rgb(255, 0, 0)").as_deref(), Some("#FF0000"));
        assert_eq!(parse_color("RGB(0,128,255)").as_deref(), Some("#0080FF"));
        assert_eq!(parse_color("rgb(0 128 255)").as_deref(), Some("#0080FF"));
        assert_eq!(
            parse_color("rgb(100%, 0%, 50%)").as_deref(),
            Some("#FF0080")
        );
        assert_eq!(
            parse_color("rgba(255, 255, 255, 0.5)").as_deref(),
            Some("#FFFFFF80")
        );
        assert_eq!(parse_color("rgba(0,0,0,1)").as_deref(), Some("#000000FF"));
        assert_eq!(
            parse_color("rgb(0 0 0 / 50%)").as_deref(),
            Some("#00000080")
        );
        assert_eq!(
            parse_color("rgb(10.4, 10.6, 0)").as_deref(),
            Some("#0A0B00")
        );
    }

    #[test]
    fn invalid_rgb_is_not_a_color() {
        for s in [
            "rgb(256, 0, 0)",
            "rgb(-1, 0, 0)",
            "rgb(1, 2)",
            "rgb(1, 2, 3, 4, 5)",
            "rgb(1, 2, 3",
            "rgb(a, b, c)",
            "rgb(inf, 0, 0)",
            "rgb(NaN, 0, 0)",
            "rgba(0, 0, 0, 2)",
            "rgb()",
            "rgb(1,,2,3)",
            "rgb(1, 2, 3) trailing",
        ] {
            assert_eq!(parse_color(s), None, "{s:?}");
        }
    }

    #[test]
    fn hsl_functions() {
        assert_eq!(parse_color("hsl(0, 100%, 50%)").as_deref(), Some("#FF0000"));
        assert_eq!(
            parse_color("hsl(120deg 100% 25%)").as_deref(),
            Some("#008000")
        );
        assert_eq!(
            parse_color("hsl(240, 100%, 50%)").as_deref(),
            Some("#0000FF")
        );
        assert_eq!(
            parse_color("hsl(-120, 100%, 50%)").as_deref(),
            Some("#0000FF")
        );
        assert_eq!(parse_color("hsl(0, 0%, 100%)").as_deref(), Some("#FFFFFF"));
        assert_eq!(
            parse_color("hsla(0, 100%, 50%, 0.5)").as_deref(),
            Some("#FF000080")
        );
    }

    #[test]
    fn invalid_hsl_is_not_a_color() {
        for s in [
            "hsl(0, 100, 50)",
            "hsl(0, 101%, 50%)",
            "hsl(0%, 100%, 50%)",
            "hsl(0, 100%)",
        ] {
            assert_eq!(parse_color(s), None, "{s:?}");
        }
    }

    // ---- classify_text ----

    #[test]
    fn colors_are_classified() {
        assert_eq!(classify_text("#1e90ff"), ClipKind::Color);
        assert_eq!(classify_text(" rgb(1, 2, 3)\n"), ClipKind::Color);
    }

    #[test]
    fn links_are_classified() {
        for s in [
            "https://example.com",
            "http://example.com/path?q=1#frag",
            "  https://github.com/tauri-apps/tauri  ",
            "www.example.com",
            "mailto:someone@example.com",
            "ftp://files.example.com/a.txt",
        ] {
            assert_eq!(classify_text(s), ClipKind::Link, "{s:?}");
        }
    }

    #[test]
    fn non_links_are_not_links() {
        for s in [
            "https://",
            "see https://example.com for details",
            "https://example.com\nhttps://example.org",
            "www.",
            "javascript:alert(1)",
            "example.com",
        ] {
            assert_ne!(classify_text(s), ClipKind::Link, "{s:?}");
        }
    }

    #[test]
    fn code_is_classified() {
        let samples = [
            "fn main() {\n    println!(\"hi\");\n}",
            "def add(a, b):\n    return a + b\n",
            "const x = 5;",
            "let total = items.iter().map(|i| i.price).sum::<u64>();",
            "import React from 'react';\nexport default function App() {\n  return <div/>;\n}",
            "#include <stdio.h>\nint main(void) { return 0; }",
            "{\n  \"name\": \"satchel\",\n  \"version\": \"0.1.0\"\n}",
            "<div class=\"a\">\n  <span>hi</span>\n</div>",
            "SELECT id, name FROM users WHERE id = 1;",
            "#!/bin/sh\necho hello",
        ];
        for s in samples {
            assert_eq!(classify_text(s), ClipKind::Code, "{s:?}");
        }
    }

    #[test]
    fn prose_is_text() {
        let samples = [
            "",
            "   ",
            "Hello world",
            "Meeting moved to 3pm; bring the slides.",
            "Dear team,\n\nThe launch is next week. Please review the plan\nand send comments by Friday.\n\nThanks!",
            "- milk\n- eggs\n- bread",
            "If you come, let me know.",
            "Return the book (the blue one) by Monday.",
            "#hashtag",
        ];
        for s in samples {
            assert_eq!(classify_text(s), ClipKind::Text, "{s:?}");
        }
    }

    #[test]
    fn classify_is_cheap_on_huge_input() {
        let big = "word ".repeat(400_000); // 2 MB
        let start = std::time::Instant::now();
        assert_eq!(classify_text(&big), ClipKind::Text);
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
    }

    // ---- preview ----

    #[test]
    fn preview_trims_and_collapses_blank_lines() {
        assert_eq!(preview("  hello  ", 300), "hello");
        assert_eq!(preview("a\n\n\n\nb", 300), "a\n\nb");
        assert_eq!(preview("a\n  \n\t\n b", 300), "a\n\n b");
        assert_eq!(preview("\n\n  a\r\nb\r\n\r\n\r\nc\n\n", 300), "a\nb\n\nc");
        assert_eq!(preview("one\ntwo", 300), "one\ntwo");
    }

    #[test]
    fn preview_truncates_on_char_boundary() {
        assert_eq!(preview("héllo wörld", 4), "héll");
        assert_eq!(preview("日本語のテキスト", 3), "日本語");
        assert_eq!(preview("🙂🙂🙂", 2), "🙂🙂");
        assert_eq!(preview("abc", 0), "");
        assert_eq!(preview("a\n\n\nb", 3), "a");
        assert_eq!(preview("a\n\n\nb", 4), "a\n\nb");
        assert!(preview(&"x".repeat(10_000), 300).chars().count() == 300);
    }

    #[test]
    fn preview_never_ends_with_whitespace() {
        assert_eq!(preview("abc   def", 5), "abc");
    }

    #[test]
    fn preview_is_cheap_on_huge_input() {
        let big = "line\n".repeat(500_000);
        let start = std::time::Instant::now();
        let p = preview(&big, 300);
        assert!(p.chars().count() <= 300);
        assert!(start.elapsed() < std::time::Duration::from_millis(20));
    }
}

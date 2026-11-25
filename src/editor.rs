use crate::theme::ThemePalette;
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{FontFamily, FontId};

/// Build a syntax-highlighted LayoutJob for LaTeX source text.
pub fn highlight_latex(text: &str, pal: ThemePalette, font_size: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font_id = FontId::new(font_size, FontFamily::Monospace);

    let normal = TextFormat { font_id: font_id.clone(), color: pal.text, ..Default::default() };
    let cmd_fmt = TextFormat { font_id: font_id.clone(), color: pal.syn_cmd, ..Default::default() };
    let env_fmt = TextFormat { font_id: font_id.clone(), color: pal.syn_env, ..Default::default() };
    let comment_fmt = TextFormat { font_id: font_id.clone(), color: pal.syn_comment, italics: true, ..Default::default() };
    let math_fmt = TextFormat { font_id: font_id.clone(), color: pal.syn_math, ..Default::default() };
    let brace_fmt = TextFormat { font_id, color: pal.syn_brace, ..Default::default() };

    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    let mut in_math = false;

    while i < n {
        let b = bytes[i];
        // Comment until end of line
        if b == b'%' && (i == 0 || bytes[i - 1] != b'\\') {
            let start = i;
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            job.append(&text[start..i], 0.0, comment_fmt.clone());
            continue;
        }
        // Math toggle ($ or $$)
        if b == b'$' && (i == 0 || bytes[i - 1] != b'\\') {
            let start = i;
            i += 1;
            if i < n && bytes[i] == b'$' {
                i += 1;
            }
            in_math = !in_math;
            job.append(&text[start..i], 0.0, math_fmt.clone());
            continue;
        }
        // LaTeX command: \identifier or single escaped char
        if b == b'\\' {
            let start = i;
            i += 1;
            if i < n && bytes[i].is_ascii_alphabetic() {
                while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'*') {
                    i += 1;
                }
                let word = &text[start..i];
                let fmt = if matches!(word, "\\begin" | "\\end" | "\\documentclass" | "\\usepackage" | "\\input" | "\\include") {
                    env_fmt.clone()
                } else {
                    cmd_fmt.clone()
                };
                job.append(word, 0.0, fmt);
            } else if i < n {
                // Advance past one UTF-8 char
                let ch_len = text[i..].chars().next().map_or(1, |c| c.len_utf8());
                i += ch_len;
                job.append(&text[start..i], 0.0, cmd_fmt.clone());
            } else {
                job.append(&text[start..i], 0.0, cmd_fmt.clone());
            }
            continue;
        }
        // Braces and brackets
        if matches!(b, b'{' | b'}' | b'[' | b']') {
            let start = i;
            i += 1;
            job.append(&text[start..i], 0.0, brace_fmt.clone());
            continue;
        }
        // Normal or math span
        let start = i;
        while i < n {
            let c = bytes[i];
            if c == b'\\' || c == b'%' || c == b'$' || matches!(c, b'{' | b'}' | b'[' | b']') {
                break;
            }
            if c == b'\n' {
                in_math = false;
            }
            let ch_len = text[i..].chars().next().map_or(1, |ch| ch.len_utf8());
            i += ch_len;
        }
        let fmt = if in_math { math_fmt.clone() } else { normal.clone() };
        job.append(&text[start..i], 0.0, fmt);
    }
    job
}

/// Apply a wrap around the current selection (or insert at cursor / end) in `text`.
/// Returns updated cursor char index.
pub fn wrap_selection(text: &mut String, sel_range: Option<(usize, usize)>, before: &str, after: &str) -> usize {
    let char_count = text.chars().count();
    let (s, e) = sel_range.unwrap_or((char_count, char_count));
    let (s, e) = (s.min(e).min(char_count), s.max(e).min(char_count));

    let byte_s = char_to_byte(text, s);
    let byte_e = char_to_byte(text, e);
    let selected = text[byte_s..byte_e].to_string();

    if selected.starts_with(before) && selected.ends_with(after) && selected.len() >= before.len() + after.len() {
        let unwrapped = &selected[before.len()..selected.len() - after.len()];
        text.replace_range(byte_s..byte_e, unwrapped);
        s + unwrapped.chars().count()
    } else {
        let wrapped = format!("{before}{selected}{after}");
        text.replace_range(byte_s..byte_e, &wrapped);
        if s == e {
            s + before.chars().count()
        } else {
            s + wrapped.chars().count()
        }
    }
}

pub fn insert_at_cursor(text: &mut String, sel_range: Option<(usize, usize)>, snippet: &str) -> usize {
    let char_count = text.chars().count();
    let (s, e) = sel_range.unwrap_or((char_count, char_count));
    let (s, e) = (s.min(e).min(char_count), s.max(e).min(char_count));
    let byte_s = char_to_byte(text, s);
    let byte_e = char_to_byte(text, e);
    text.replace_range(byte_s..byte_e, snippet);
    s + snippet.chars().count()
}

pub fn toggle_comment(text: &mut String, sel_range: Option<(usize, usize)>) {
    let char_count = text.chars().count();
    let (s, e) = sel_range.unwrap_or((0, 0));
    let byte_s = char_to_byte(text, s.min(e).min(char_count));
    let byte_e = char_to_byte(text, s.max(e).min(char_count));

    // Find full line boundaries
    let line_start = text[..byte_s].rfind('\n').map_or(0, |p| p + 1);
    let line_end = text[byte_e..].find('\n').map_or(text.len(), |p| byte_e + p);
    let block = text[line_start..line_end].to_string();
    let lines: Vec<&str> = block.split('\n').collect();
    let all_commented = lines.iter().all(|l| l.trim_start().starts_with('%'));

    let new_lines: Vec<String> = lines
        .into_iter()
        .map(|l| {
            if all_commented {
                if let Some(pos) = l.find('%') {
                    let mut s = String::with_capacity(l.len());
                    s.push_str(&l[..pos]);
                    let rest = &l[pos + 1..];
                    s.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                    s
                } else {
                    l.to_string()
                }
            } else {
                format!("% {l}")
            }
        })
        .collect();

    text.replace_range(line_start..line_end, &new_lines.join("\n"));
}

pub fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
}

pub fn line_col_of_char(s: &str, char_idx: usize) -> (usize, usize) {
    let b = char_to_byte(s, char_idx);
    let prefix = &s[..b];
    let line = prefix.bytes().filter(|&c| c == b'\n').count() + 1;
    let col = prefix.rfind('\n').map_or(prefix.chars().count(), |p| prefix[p + 1..].chars().count()) + 1;
    (line, col)
}

pub fn char_of_line(s: &str, target_line_1based: usize) -> usize {
    if target_line_1based <= 1 {
        return 0;
    }
    let mut line = 1;
    for (idx, ch) in s.chars().enumerate() {
        if line == target_line_1based {
            return idx;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    s.chars().count()
}

pub fn ensure_package(text: &mut String, pkg: &str) {
    let needle = format!("{{{pkg}}}");
    if text.contains(&needle) {
        return;
    }
    if let Some(pos) = text.find("\\begin{document}") {
        let ins = format!("\\usepackage{{{pkg}}}\n");
        text.insert_str(pos, &ins);
    }
}

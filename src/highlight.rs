//! Syntax highlighting with syntect, run off the UI thread.
//!
//! The SyntaxSet (two-face's bat set: syntect defaults + TS/TSX/TOML/Dockerfile/…) is
//! deserialized once, lazily, on a worker thread.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use syntect::highlighting::{Color, FontStyle, HighlightIterator, HighlightState, Highlighter, Theme};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// Files above this are shown as plain text (lazy highlighting comes in phase 1).
pub const MAX_HIGHLIGHT_BYTES: usize = 1 << 20;
pub const MAX_HIGHLIGHT_LINES: usize = 20_000;
/// Minified / generated files: syntect is slow on very long lines.
const MAX_LINE_BYTES: usize = 5_000;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

pub fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

/// Start deserializing the syntax set in the background.
pub fn preload() {
    std::thread::spawn(|| {
        syntaxes();
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SpanStyle {
    pub fg: (u8, u8, u8, u8),
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// One styled run: line number, byte range within the line, index into `styles`.
pub struct Span {
    pub line: u32,
    pub start: u32,
    pub end: u32,
    pub style: u16,
}

pub struct Highlighted {
    pub syntax_name: String,
    pub styles: Vec<SpanStyle>,
    pub spans: Vec<Span>,
}

pub fn find_syntax<'a>(ss: &'a SyntaxSet, path: &Path, text: &str) -> &'a SyntaxReference {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    ss.find_syntax_by_extension(name)
        .or_else(|| ss.find_syntax_by_extension(ext))
        .or_else(|| ss.find_syntax_by_extension(&ext.to_ascii_lowercase()))
        .or_else(|| ss.find_syntax_by_first_line(text.lines().next().unwrap_or_default()))
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

/// VS Code's bracket pair colorization colors (RGBA): 3 by nesting depth + unmatched.
pub struct BracketColors {
    pub cycle: [(u8, u8, u8, u8); 3],
    pub unexpected: (u8, u8, u8, u8),
}

#[derive(Default)]
struct StyleTable {
    styles: Vec<SpanStyle>,
    index: HashMap<SpanStyle, u16>,
}

impl StyleTable {
    fn id(&mut self, s: SpanStyle) -> u16 {
        let next = self.styles.len() as u16;
        *self.index.entry(s).or_insert_with(|| {
            self.styles.push(s);
            next
        })
    }
}

fn color_only(fg: (u8, u8, u8, u8)) -> SpanStyle {
    SpanStyle { fg, bold: false, italic: false, underline: false }
}

/// Highlight `text`. Spans in the default foreground with no font style are omitted:
/// the view's base color already covers them. Brackets outside strings/comments get
/// VS Code's depth colors.
pub fn highlight(text: &str, path: &Path, theme: &Theme, brackets: &BracketColors) -> Highlighted {
    let ss = syntaxes();
    let syntax = find_syntax(ss, path, text);
    let mut out = Highlighted { syntax_name: syntax.name.clone(), styles: Vec::new(), spans: Vec::new() };
    let too_big = text.len() > MAX_HIGHLIGHT_BYTES
        || text.lines().count() > MAX_HIGHLIGHT_LINES
        || text.lines().any(|l| l.len() > MAX_LINE_BYTES);
    if too_big || syntax.name == "Plain Text" {
        return out;
    }

    let default_fg = theme.settings.foreground.unwrap_or(Color::WHITE);
    let highlighter = Highlighter::new(theme);
    let mut parse = ParseState::new(syntax);
    let mut hl_state = HighlightState::new(&highlighter, ScopeStack::new());
    let mut scopes = ScopeStack::new();
    let no_brackets = [Scope::new("comment").unwrap(), Scope::new("string").unwrap()];
    let mut depth = 0usize;
    let mut table = StyleTable::default();

    for (line_no, line) in LinesWithEndings::from(text).enumerate() {
        let line_no = line_no as u32;
        let Ok(ops) = parse.parse_line(line, ss) else { break };

        // Bracket positions on this line and their colors.
        let mut marks: Vec<(usize, (u8, u8, u8, u8))> = Vec::new();
        let mut op_i = 0;
        for (i, b) in line.bytes().enumerate() {
            while op_i < ops.len() && ops[op_i].0 <= i {
                let _ = scopes.apply(&ops[op_i].1);
                op_i += 1;
            }
            let open = matches!(b, b'(' | b'[' | b'{');
            if !open && !matches!(b, b')' | b']' | b'}') {
                continue;
            }
            if scopes.as_slice().iter().any(|s| no_brackets.iter().any(|n| n.is_prefix_of(*s))) {
                continue;
            }
            if open {
                marks.push((i, brackets.cycle[depth % 3]));
                depth += 1;
            } else if depth == 0 {
                marks.push((i, brackets.unexpected));
            } else {
                depth -= 1;
                marks.push((i, brackets.cycle[depth % 3]));
            }
        }
        for (_, op) in &ops[op_i..] {
            let _ = scopes.apply(op);
        }

        // Token styles, split around bracket marks so spans never overlap.
        let mut push = |table: &mut StyleTable, start: usize, end: usize, style: SpanStyle| {
            let end = if line[..end].ends_with('\n') { end - 1 } else { end };
            if end > start && !line[start..end].trim().is_empty() {
                let id = table.id(style);
                out.spans.push(Span { line: line_no, start: start as u32, end: end as u32, style: id });
            }
        };
        let mut offset = 0usize;
        let mut m = 0usize;
        for (style, piece) in HighlightIterator::new(&mut hl_state, &ops, line, &highlighter) {
            let end = offset + piece.len();
            let fs = style.font_style;
            let token = (style.foreground != default_fg || !fs.is_empty()).then(|| SpanStyle {
                fg: (style.foreground.r, style.foreground.g, style.foreground.b, style.foreground.a),
                bold: fs.contains(FontStyle::BOLD),
                italic: fs.contains(FontStyle::ITALIC),
                underline: fs.contains(FontStyle::UNDERLINE),
            });
            let mut pos = offset;
            while m < marks.len() && marks[m].0 < end {
                let (bi, color) = marks[m];
                if let Some(t) = token {
                    push(&mut table, pos, bi, t);
                }
                push(&mut table, bi, bi + 1, color_only(color));
                pos = bi + 1;
                m += 1;
            }
            if let Some(t) = token {
                push(&mut table, pos, end, t);
            }
            offset = end;
        }
    }
    out.styles = table.styles;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets_skip_strings_and_cycle() {
        let theme = crate::theme::ThemeModel::load(crate::theme::DEFAULT_THEME).unwrap();
        let colors = theme.bracket_colors();
        let src = "fn f() { let s = \"(\"; g([1]); }\n";
        let hl = highlight(src, Path::new("a.rs"), &theme.to_syntect(), &colors);
        let at = |i: u32| {
            hl.spans.iter().find(|s| s.start == i && s.end == i + 1).map(|s| hl.styles[s.style as usize].fg)
        };
        assert_eq!(at(4), Some(colors.cycle[0])); // (
        assert_eq!(at(7), Some(colors.cycle[0])); // {
        assert_ne!(at(18), Some(colors.cycle[1])); // "(" inside a string
        assert_eq!(at(23), Some(colors.cycle[1])); // g(
        assert_eq!(at(24), Some(colors.cycle[2])); // [
    }
}

#[cfg(test)]
mod mem {
    fn rss_kb() -> u64 {
        let s = std::fs::read_to_string("/proc/self/status").unwrap();
        s.lines().find(|l| l.starts_with("VmRSS")).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap()
    }

    #[test]
    #[ignore]
    fn syntax_set_cost() {
        let before = rss_kb();
        let t = std::time::Instant::now();
        let ss = super::syntaxes();
        let load = t.elapsed();
        let after = rss_kb();
        let t = std::time::Instant::now();
        let theme = crate::theme::ThemeModel::load(crate::theme::DEFAULT_THEME).unwrap();
        let src = std::fs::read_to_string("src/codeview.rs").unwrap();
        let hl = super::highlight(&src, std::path::Path::new("x.rs"), &theme.to_syntect(), &theme.bracket_colors());
        eprintln!(
            "syntaxes: {} in {:?}, +{} MB RSS; highlight codeview.rs: {} spans in {:?}, RSS now +{} MB",
            ss.syntaxes().len(), load, (after - before) / 1024, hl.spans.len(), t.elapsed(), (rss_kb() - before) / 1024
        );
    }
}

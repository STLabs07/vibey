//! Syntax highlighting with irosashi: VS Code's TextMate grammars and tokenizer semantics,
//! so token colors match VS Code. Runs off the UI thread.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use irosashi::{CodeToTokensOptions, FontStyle, Highlighter};

use crate::theme::Rgba;

/// Larger files are shown as plain text.
pub const MAX_HIGHLIGHT_BYTES: usize = 4 << 20;
/// Longer lines (minified files) are emitted unstyled by irosashi.
const MAX_LINE_BYTES: usize = 10_000;

static HIGHLIGHTER: OnceLock<Highlighter> = OnceLock::new();

pub fn highlighter() -> &'static Highlighter {
    HIGHLIGHTER.get_or_init(|| Highlighter::new().expect("embedded grammars"))
}

/// Initialize the highlighter in the background.
pub fn preload() {
    std::thread::spawn(|| {
        highlighter();
    });
}

pub fn register_theme(id: &str, json: Vec<u8>) -> Result<(), String> {
    highlighter().load_theme(id, json).map_err(|e| e.to_string())
}

/// irosashi language id for a file.
pub fn detect(path: &Path, text: &str) -> Option<String> {
    let h = highlighter();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let first = text.lines().next().unwrap_or_default();
    h.detect_language(name)
        .or_else(|| h.detect_language(&name.to_lowercase()))
        .or_else(|| h.detect_language_by_first_line(first))
        .or_else(|| shebang(first).map(str::to_string))
}

/// Language from a `#!` line (`#!/bin/bash`, `#!/usr/bin/env python3`), as VS Code does.
fn shebang(line: &str) -> Option<&'static str> {
    let rest = line.strip_prefix("#!")?;
    let mut words = rest.split_whitespace();
    let mut prog = words.next()?.rsplit('/').next()?;
    if prog == "env" {
        prog = words.find(|w| !w.starts_with('-'))?;
    }
    let base = prog.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    Some(match base {
        "sh" | "bash" | "zsh" | "dash" | "ksh" => "shellscript",
        "python" => "python",
        "node" | "deno" | "bun" => "javascript",
        "ruby" => "ruby",
        "perl" => "perl",
        "php" => "php",
        "lua" => "lua",
        "fish" => "fish",
        _ => return None,
    })
}

/// The name VS Code shows in the status bar.
pub fn display_name(lang: Option<&str>) -> String {
    let Some(lang) = lang else { return "Plain Text".into() };
    let known = match lang {
        "rust" => "Rust",
        "typescript" | "ts" => "TypeScript",
        "tsx" => "TypeScript JSX",
        "javascript" | "js" => "JavaScript",
        "jsx" => "JavaScript JSX",
        "python" | "py" => "Python",
        "json" => "JSON",
        "jsonc" => "JSON with Comments",
        "jsonl" => "JSON Lines",
        "markdown" | "md" => "Markdown",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "html" => "HTML",
        "css" => "CSS",
        "scss" => "SCSS",
        "less" => "Less",
        "shellscript" | "bash" | "sh" | "zsh" | "shell" => "Shell Script",
        "go" => "Go",
        "c" => "C",
        "cpp" => "C++",
        "csharp" => "C#",
        "java" => "Java",
        "kotlin" => "Kotlin",
        "docker" | "dockerfile" => "Dockerfile",
        "make" | "makefile" => "Makefile",
        "sql" => "SQL",
        "xml" => "XML",
        "php" => "PHP",
        "ruby" => "Ruby",
        "lua" => "Lua",
        "swift" => "Swift",
        "dart" => "Dart",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "ini" => "Ini",
        "diff" => "Diff",
        "dotenv" => "Dotenv",
        "graphql" => "GraphQL",
        "nix" => "Nix",
        "zig" => "Zig",
        "elixir" => "Elixir",
        "haskell" => "Haskell",
        "ocaml" => "OCaml",
        _ => "",
    };
    if !known.is_empty() {
        return known.into();
    }
    let mut c = lang.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SpanStyle {
    pub fg: (u8, u8, u8, u8),
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// One styled run: line number, byte range within the line, index into `styles`.
pub struct Span {
    pub line: u32,
    pub start: u32,
    pub end: u32,
    pub style: u16,
}

pub struct Highlighted {
    pub language: String,
    pub styles: Vec<SpanStyle>,
    pub spans: Vec<Span>,
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
    SpanStyle { fg, bold: false, italic: false, underline: false, strikethrough: false }
}

/// Highlight `text` with theme `theme_id`. Spans in the default foreground with no font
/// style are omitted: the view's base color already covers them. Brackets outside
/// strings/comments get VS Code's depth colors.
pub fn highlight(text: &str, path: &Path, theme_id: &str, brackets: &BracketColors) -> Highlighted {
    let lang = detect(path, text);
    let mut out = Highlighted { language: display_name(lang.as_deref()), styles: Vec::new(), spans: Vec::new() };
    let Some(lang) = lang else { return out };
    if text.len() > MAX_HIGHLIGHT_BYTES {
        return out;
    }
    let mut opts = CodeToTokensOptions::new(&lang, theme_id);
    opts.include_scopes = true;
    opts.max_line_length = Some(MAX_LINE_BYTES);
    let Ok(res) = highlighter().code_to_tokens(text, &opts) else { return out };

    let default_fg = Rgba::parse(res.fg());
    let mut table = StyleTable::default();
    let mut depth = 0usize;
    for (line_no, line) in res.lines.iter().enumerate() {
        let line_no = line_no as u32;
        let ltext = res.line_text(line);
        let mut push = |table: &mut StyleTable, start: usize, end: usize, style: SpanStyle| {
            if end > start && !ltext[start..end].trim().is_empty() {
                let id = table.id(style);
                out.spans.push(Span { line: line_no, start: start as u32, end: end as u32, style: id });
            }
        };
        for tok in &line.tokens {
            let fg = tok.style.color.and_then(|c| Rgba::parse(res.color(c)));
            let fs = tok.style.font_style;
            let token = (fg.is_some() && fg != default_fg || !fs.is_empty()).then(|| {
                let c = fg.or(default_fg).unwrap_or(Rgba { r: 255, g: 255, b: 255, a: 255 });
                SpanStyle {
                    fg: (c.r, c.g, c.b, c.a),
                    bold: fs.contains(FontStyle::BOLD),
                    italic: fs.contains(FontStyle::ITALIC),
                    underline: fs.contains(FontStyle::UNDERLINE),
                    strikethrough: fs.contains(FontStyle::STRIKETHROUGH),
                }
            });

            let no_brackets = res.scopes.as_ref().and_then(|t| t.get(tok.scopes)).is_some_and(|scopes| {
                scopes.iter().any(|s| s.starts_with("comment") || s.starts_with("string"))
            });
            // Split the token around bracket characters so spans never overlap.
            let mut pos = tok.start;
            if !no_brackets {
                for (i, b) in ltext[tok.start..tok.end].bytes().enumerate() {
                    let open = matches!(b, b'(' | b'[' | b'{');
                    if !open && !matches!(b, b')' | b']' | b'}') {
                        continue;
                    }
                    let color = if open {
                        depth += 1;
                        brackets.cycle[(depth - 1) % 3]
                    } else if depth == 0 {
                        brackets.unexpected
                    } else {
                        depth -= 1;
                        brackets.cycle[depth % 3]
                    };
                    let at = tok.start + i;
                    if let Some(t) = token {
                        push(&mut table, pos, at, t);
                    }
                    push(&mut table, at, at + 1, color_only(color));
                    pos = at + 1;
                }
            }
            if let Some(t) = token {
                push(&mut table, pos, tok.end, t);
            }
        }
    }
    out.styles = table.styles;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeModel, DEFAULT_THEME};

    fn color_at(hl: &Highlighted, line: u32, i: u32) -> Option<(u8, u8, u8, u8)> {
        hl.spans
            .iter()
            .find(|s| s.line == line && s.start <= i && i < s.end)
            .map(|s| hl.styles[s.style as usize].fg)
    }

    #[test]
    fn brackets_skip_strings_and_cycle() {
        let theme = ThemeModel::load(DEFAULT_THEME);
        let colors = theme.bracket_colors();
        let src = "fn f() { let s = \"(\"; g([1]); }\n";
        let hl = highlight(src, Path::new("a.rs"), DEFAULT_THEME, &colors);
        assert_eq!(hl.language, "Rust");
        assert_eq!(color_at(&hl, 0, 4), Some(colors.cycle[0])); // (
        assert_eq!(color_at(&hl, 0, 7), Some(colors.cycle[0])); // {
        assert_ne!(color_at(&hl, 0, 18), Some(colors.cycle[1])); // "(" inside a string
        assert_eq!(color_at(&hl, 0, 23), Some(colors.cycle[1])); // g(
        assert_eq!(color_at(&hl, 0, 24), Some(colors.cycle[2])); // [
    }

    #[test]
    fn matches_vscode_dark_2026() {
        // Colors sampled from VS Code 1.141 (Dark 2026, no extensions) on the same code.
        let theme = ThemeModel::load(DEFAULT_THEME);
        let src = "//! doc\nuse std::collections::HashMap;\nconst MAX: u64 = 20;\n";
        let hl = highlight(src, Path::new("a.rs"), DEFAULT_THEME, &theme.bracket_colors());
        let hex = |c: Option<(u8, u8, u8, u8)>| c.map(|(r, g, b, _)| format!("#{r:02x}{g:02x}{b:02x}"));
        assert_eq!(hex(color_at(&hl, 0, 4)).as_deref(), Some("#8b949e")); // doc comment body
        assert_eq!(hex(color_at(&hl, 1, 0)).as_deref(), Some("#ff7b72")); // use
        assert_eq!(hex(color_at(&hl, 1, 22)).as_deref(), Some("#4ec9b0")); // HashMap
        assert_eq!(hex(color_at(&hl, 2, 6)).as_deref(), Some("#79c0ff")); // MAX
        assert_eq!(hex(color_at(&hl, 2, 11)).as_deref(), Some("#4ec9b0")); // u64
    }

    #[test]
    fn detects_common_languages() {
        let lang = |n: &str| display_name(detect(Path::new(n), "").as_deref());
        assert_eq!(lang("a.tsx"), "TypeScript JSX");
        assert_eq!(lang("Dockerfile"), "Dockerfile");
        assert_eq!(lang("Cargo.toml"), "TOML");
        assert_eq!(lang("notes.unknownext"), "Plain Text");
        assert_eq!(display_name(detect(Path::new("run"), "#!/bin/bash\n").as_deref()), "Shell Script");
    }
}

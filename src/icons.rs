//! VS Code's default file icon theme (Seti: an icon font + a name/extension mapping) and
//! its UI icon font (Codicon).
//!
//! The font is registered with fontconfig as an application font before GTK starts, so
//! labels can render glyphs with `font-family: seti` / `codicon`. Seti ships as TTF (converted from
//! VS Code's WOFF by scripts/woff2ttf.py) because HarfBuzz, which Pango uses, can't read WOFF.

use std::collections::HashMap;
use std::ffi::{c_int, c_void, CString};
use std::sync::OnceLock;

use serde_json::Value;

const SETI_FONT: &[u8] = include_bytes!("../assets/icons/seti.ttf");
const CODICON_FONT: &[u8] = include_bytes!("../assets/icons/codicon.ttf");

/// Codicon glyphs (from VS Code's codicon.css).
pub mod codicon {
    pub const CHEVRON_RIGHT: &str = "\u{eab6}";
    pub const CHEVRON_DOWN: &str = "\u{eab4}";
    pub const CLOSE: &str = "\u{ea76}";
    pub const GIT_BRANCH: &str = "\u{ea68}";
    pub const ARROW_UP: &str = "\u{eaa1}";
    pub const ARROW_DOWN: &str = "\u{ea9a}";
    pub const CASE_SENSITIVE: &str = "\u{eab1}";
    pub const ADD: &str = "\u{ea60}";
    pub const SETTINGS_GEAR: &str = "\u{eb51}";
    pub const TERMINAL: &str = "\u{ea85}";
    pub const FOLDER_OPENED: &str = "\u{eaf7}";
    pub const CIRCLE_FILLED: &str = "\u{ea71}";
    pub const PULSE: &str = "\u{eb31}";
    pub const SEARCH: &str = "\u{ea6d}";
    pub const WARNING: &str = "\u{ea6c}";
}
const SETI_MAP: &str = include_str!("../assets/icons/vs-seti-icon-theme.json");

#[link(name = "fontconfig")]
unsafe extern "C" {
    fn FcConfigAppFontAddFile(config: *mut c_void, file: *const u8) -> c_int;
}

/// Write the bundled fonts to the cache dir (once) and register them. Call before GTK init.
pub fn register_fonts() {
    let dir = crate::cache_dir();
    for (name, data) in [("seti", SETI_FONT), ("codicon", CODICON_FONT)] {
        let path = dir.join(format!("{name}-{}.ttf", data.len()));
        if !path.exists() {
            let _ = std::fs::create_dir_all(&dir);
            if std::fs::write(&path, data).is_err() {
                continue;
            }
        }
        if let Ok(c) = CString::new(path.to_string_lossy().as_bytes()) {
            // NULL = fontconfig's current config, which Pango's font map picks up.
            unsafe {
                FcConfigAppFontAddFile(std::ptr::null_mut(), c.as_ptr() as *const u8);
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct Icon {
    pub glyph: char,
    pub color: (u8, u8, u8),
}

struct Seti {
    defs: HashMap<String, Icon>,
    names: HashMap<String, String>,
    exts: HashMap<String, String>,
    langs: HashMap<String, String>,
    default: String,
}

fn seti() -> &'static Seti {
    static SETI: OnceLock<Seti> = OnceLock::new();
    SETI.get_or_init(|| {
        let v: Value = serde_json::from_str(SETI_MAP).expect("bundled seti map");
        let map = |key: &str| -> HashMap<String, String> {
            v[key]
                .as_object()
                .map(|o| o.iter().filter_map(|(k, v)| Some((k.to_lowercase(), v.as_str()?.to_string()))).collect())
                .unwrap_or_default()
        };
        let mut defs = HashMap::new();
        for (id, d) in v["iconDefinitions"].as_object().into_iter().flatten() {
            let glyph = d["fontCharacter"]
                .as_str()
                .and_then(|s| u32::from_str_radix(s.trim_start_matches('\\'), 16).ok())
                .and_then(char::from_u32);
            let color = d["fontColor"].as_str().and_then(crate::theme::Rgba::parse);
            if let (Some(glyph), Some(c)) = (glyph, color) {
                defs.insert(id.clone(), Icon { glyph, color: (c.r, c.g, c.b) });
            }
        }
        Seti {
            defs,
            names: map("fileNames"),
            exts: map("fileExtensions"),
            langs: map("languageIds"),
            default: v["file"].as_str().unwrap_or("_default").to_string(),
        }
    })
}

/// VS Code's language id for common file types (Seti maps many icons by language).
fn language_id(name: &str, ext: &str) -> Option<&'static str> {
    Some(match name {
        "dockerfile" => "dockerfile",
        "makefile" | "gnumakefile" => "makefile",
        ".gitignore" | ".dockerignore" | ".npmignore" => "ignore",
        ".env" => "dotenv",
        _ => match ext {
            "rs" => "rust",
            "py" | "pyi" => "python",
            "ts" | "mts" | "cts" => "typescript",
            "tsx" => "typescriptreact",
            "js" | "mjs" | "cjs" => "javascript",
            "jsx" => "javascriptreact",
            "json" => "json",
            "jsonc" => "jsonc",
            "md" | "markdown" => "markdown",
            "html" | "htm" => "html",
            "css" => "css",
            "scss" => "scss",
            "less" => "less",
            "sh" | "bash" | "zsh" => "shellscript",
            "go" => "go",
            "java" => "java",
            "c" | "h" => "c",
            "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
            "cs" => "csharp",
            "rb" => "ruby",
            "php" => "php",
            "yml" | "yaml" => "yaml",
            "xml" => "xml",
            "sql" => "sql",
            "lua" => "lua",
            "swift" => "swift",
            "dart" => "dart",
            "r" => "r",
            "pl" => "perl",
            "ps1" => "powershell",
            "bat" | "cmd" => "bat",
            "ex" | "exs" => "elixir",
            "jl" => "julia",
            "tex" => "latex",
            _ => return None,
        },
    })
}

/// Icon for a file name, using VS Code's lookup order: file name, extensions
/// (longest first), language id, default.
pub fn for_file(name: &str) -> Icon {
    let s = seti();
    let lower = name.to_lowercase();
    let mut id = s.names.get(&lower);
    if id.is_none() {
        let mut rest = lower.as_str();
        while let Some(dot) = rest.find('.') {
            rest = &rest[dot + 1..];
            if let Some(found) = s.exts.get(rest) {
                id = Some(found);
                break;
            }
        }
    }
    if id.is_none() {
        let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or_default();
        id = language_id(&lower, ext).and_then(|l| s.langs.get(l));
    }
    let id = id.unwrap_or(&s.default);
    s.defs.get(id).or_else(|| s.defs.get(&s.default)).copied().unwrap_or(Icon { glyph: ' ', color: (0, 0, 0) })
}

/// Pango markup for the icon glyph.
pub fn markup(icon: Icon) -> String {
    let (r, g, b) = icon.color;
    format!("<span foreground=\"#{r:02x}{g:02x}{b:02x}\">{}</span>", icon.glyph)
}

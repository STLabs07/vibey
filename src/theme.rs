//! VS Code color theme loader.
//!
//! Reads a VS Code theme JSON (JSONC, with `include` chains), and converts it into:
//! - a syntect `Theme` for token colors,
//! - GTK CSS `@define-color`s for workbench colors,
//! - a VTE palette for the terminal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_json::Value;
use syntect::highlighting::{
    Color, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};

/// Themes shipped inside the binary, addressed by file name (so `include` works).
const BUILTIN: &[(&str, &str)] = &[
    ("2026-dark.json", include_str!("../assets/themes/2026-dark.json")),
    ("dark_modern.json", include_str!("../assets/themes/dark_modern.json")),
    ("dark_plus.json", include_str!("../assets/themes/dark_plus.json")),
    ("dark_vs.json", include_str!("../assets/themes/dark_vs.json")),
];

pub const DEFAULT_THEME: &str = "2026-dark";

/// VS Code's built-in defaults (dark) for keys that themes commonly leave unset.
const DARK_DEFAULTS: &[(&str, &str)] = &[
    ("editor.background", "#1E1E1E"),
    ("editor.foreground", "#D4D4D4"),
    ("foreground", "#CCCCCC"),
    ("tree.indentGuidesStroke", "#585858"),
    ("terminal.foreground", "#CCCCCC"),
    ("terminal.ansiBlack", "#000000"),
    ("terminal.ansiRed", "#CD3131"),
    ("terminal.ansiGreen", "#0DBC79"),
    ("terminal.ansiYellow", "#E5E510"),
    ("terminal.ansiBlue", "#2472C8"),
    ("terminal.ansiMagenta", "#BC3FBC"),
    ("terminal.ansiCyan", "#11A8CD"),
    ("terminal.ansiWhite", "#E5E5E5"),
    ("terminal.ansiBrightBlack", "#666666"),
    ("terminal.ansiBrightRed", "#F14C4C"),
    ("terminal.ansiBrightGreen", "#23D18B"),
    ("terminal.ansiBrightYellow", "#F5F543"),
    ("terminal.ansiBrightBlue", "#3B8EEA"),
    ("terminal.ansiBrightMagenta", "#D670D6"),
    ("terminal.ansiBrightCyan", "#29B8DB"),
    ("terminal.ansiBrightWhite", "#E5E5E5"),
    ("terminalCursor.foreground", "#AEAFAD"),
    ("terminal.selectionBackground", "#264F78"),
    ("gitDecoration.modifiedResourceForeground", "#E2C08D"),
    ("gitDecoration.untrackedResourceForeground", "#73C991"),
    ("gitDecoration.addedResourceForeground", "#81B88B"),
    ("gitDecoration.deletedResourceForeground", "#C74E39"),
    ("gitDecoration.ignoredResourceForeground", "#8C8C8C"),
    ("editorGutter.addedBackground", "#2EA043"),
    ("editorGutter.modifiedBackground", "#0078D4"),
    ("editorGutter.deletedBackground", "#F85149"),
];

/// Workbench color keys exported to CSS as `@vs_<key with . → _>`.
const CSS_KEYS: &[&str] = &[
    "foreground",
    "focusBorder",
    "editor.background",
    "editor.foreground",
    "editor.selectionBackground",
    "editor.inactiveSelectionBackground",
    "editorLineNumber.foreground",
    "editorLineNumber.activeForeground",
    "editorGroupHeader.tabsBackground",
    "editorGroupHeader.tabsBorder",
    "tab.activeBackground",
    "tab.inactiveBackground",
    "tab.activeForeground",
    "tab.inactiveForeground",
    "tab.border",
    "tab.activeBorderTop",
    "breadcrumb.foreground",
    "breadcrumb.background",
    "sideBar.background",
    "sideBar.foreground",
    "sideBar.border",
    "sideBarTitle.foreground",
    "sideBarSectionHeader.background",
    "sideBarSectionHeader.foreground",
    "activityBar.background",
    "activityBar.foreground",
    "activityBar.inactiveForeground",
    "activityBar.border",
    "activityBar.activeBorder",
    "activityBarBadge.background",
    "activityBarBadge.foreground",
    "list.activeSelectionBackground",
    "list.activeSelectionForeground",
    "list.inactiveSelectionBackground",
    "list.hoverBackground",
    "list.focusOutline",
    "tree.indentGuidesStroke",
    "panel.background",
    "panel.border",
    "panelTitle.activeForeground",
    "panelTitle.activeBorder",
    "panelTitle.inactiveForeground",
    "terminal.background",
    "statusBar.background",
    "statusBar.foreground",
    "statusBar.border",
    "scrollbarSlider.background",
    "scrollbarSlider.hoverBackground",
    "scrollbarSlider.activeBackground",
    "gitDecoration.modifiedResourceForeground",
    "gitDecoration.untrackedResourceForeground",
    "gitDecoration.addedResourceForeground",
    "gitDecoration.deletedResourceForeground",
    "gitDecoration.ignoredResourceForeground",
    "editorGutter.addedBackground",
    "editorGutter.modifiedBackground",
    "editorGutter.deletedBackground",
    "badge.background",
    "badge.foreground",
    "descriptionForeground",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub fn parse(s: &str) -> Option<Rgba> {
        let h = s.trim().strip_prefix('#')?;
        let nib = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17);
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(match h.len() {
            3 => Rgba { r: nib(0)?, g: nib(1)?, b: nib(2)?, a: 255 },
            4 => Rgba { r: nib(0)?, g: nib(1)?, b: nib(2)?, a: nib(3)? },
            6 => Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: 255 },
            8 => Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: byte(6)? },
            _ => return None,
        })
    }

    pub fn css(&self) -> String {
        format!("rgba({},{},{},{:.3})", self.r, self.g, self.b, self.a as f32 / 255.0)
    }

    pub fn gdk(&self) -> gtk::gdk::RGBA {
        gtk::gdk::RGBA::new(
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        )
    }

    fn syntect(&self) -> Color {
        Color { r: self.r, g: self.g, b: self.b, a: self.a }
    }
}

pub struct ThemeModel {
    pub name: String,
    colors: HashMap<String, String>,
    /// tokenColors rules, included (parent) themes first.
    token_rules: Vec<Value>,
}

impl ThemeModel {
    /// Load by name (file stem, e.g. "2026-dark") or path. User themes in
    /// `~/.config/vibey/themes/` take precedence over built-in ones.
    pub fn load(spec: &str) -> Result<ThemeModel, String> {
        let file = if spec.ends_with(".json") { spec.to_string() } else { format!("{spec}.json") };
        let path = if Path::new(&file).is_absolute() {
            PathBuf::from(&file)
        } else {
            user_theme_dir().join(&file)
        };
        let mut model = ThemeModel { name: String::new(), colors: HashMap::new(), token_rules: Vec::new() };
        model.load_into(&path, 0)?;
        Ok(model)
    }

    fn load_into(&mut self, path: &Path, depth: u32) -> Result<(), String> {
        if depth > 8 {
            return Err("theme include chain too deep".into());
        }
        let text = read_theme_source(path)?;
        let json: Value = serde_json::from_str(&strip_jsonc(&text))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(inc) = json.get("include").and_then(Value::as_str) {
            let inc_path = path.parent().unwrap_or(Path::new(".")).join(inc);
            self.load_into(&inc_path, depth + 1)?;
        }
        if let Some(name) = json.get("name").and_then(Value::as_str) {
            self.name = name.to_string();
        }
        if let Some(colors) = json.get("colors").and_then(Value::as_object) {
            for (k, v) in colors {
                if let Some(s) = v.as_str() {
                    self.colors.insert(k.clone(), s.to_string());
                }
            }
        }
        if let Some(rules) = json.get("tokenColors").and_then(Value::as_array) {
            self.token_rules.extend(rules.iter().cloned());
        }
        Ok(())
    }

    pub fn color(&self, key: &str) -> Option<Rgba> {
        self.colors
            .get(key)
            .map(String::as_str)
            .or_else(|| DARK_DEFAULTS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v))
            .and_then(Rgba::parse)
    }

    pub fn color_or(&self, key: &str, fallback: &str) -> Rgba {
        self.color(key).or_else(|| Rgba::parse(fallback)).unwrap()
    }

    pub fn to_syntect(&self) -> Theme {
        let mut settings = ThemeSettings {
            foreground: self.color("editor.foreground").map(|c| c.syntect()),
            background: self.color("editor.background").map(|c| c.syntect()),
            ..Default::default()
        };
        let mut scopes = Vec::new();
        for rule in &self.token_rules {
            let s = &rule["settings"];
            let scope = match &rule["scope"] {
                Value::String(s) => s.clone(),
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
                _ => {
                    // Scope-less rule: global editor settings (old-style themes).
                    if let Some(fg) = s["foreground"].as_str().and_then(Rgba::parse) {
                        settings.foreground = Some(fg.syntect());
                    }
                    continue;
                }
            };
            let Ok(selectors) = ScopeSelectors::from_str(&scope) else { continue };
            let style = StyleModifier {
                foreground: s["foreground"].as_str().and_then(Rgba::parse).map(|c| c.syntect()),
                background: None,
                font_style: s["fontStyle"].as_str().map(parse_font_style),
            };
            if style.foreground.is_none() && style.font_style.is_none() {
                continue;
            }
            scopes.push(ThemeItem { scope: selectors, style });
        }
        Theme { name: Some(self.name.clone()), author: None, settings, scopes }
    }

    /// `@define-color` lines for every workbench key the stylesheet uses.
    pub fn css_colors(&self) -> String {
        let mut out = String::new();
        for key in CSS_KEYS {
            let c = self.color(key).unwrap_or_else(|| self.fallback_for(key));
            out.push_str(&format!("@define-color vs_{} {};\n", key.replace('.', "_"), c.css()));
        }
        out
    }

    /// VS Code derives some unset keys from others; mirror the ones we use.
    fn fallback_for(&self, key: &str) -> Rgba {
        let from = match key {
            "terminal.background" => "panel.background",
            "breadcrumb.background" => "editor.background",
            "sideBarSectionHeader.foreground" | "sideBarTitle.foreground" => "sideBar.foreground",
            "editor.inactiveSelectionBackground" => "editor.selectionBackground",
            "scrollbarSlider.activeBackground" => "scrollbarSlider.hoverBackground",
            "activityBar.inactiveForeground" | "descriptionForeground" => "editorLineNumber.foreground",
            "badge.foreground" | "activityBarBadge.foreground" => "editor.foreground",
            "activityBarBadge.background" | "activityBar.activeBorder" => "focusBorder",
            _ => "foreground",
        };
        self.color(from).unwrap_or(Rgba { r: 0xcc, g: 0xcc, b: 0xcc, a: 0xff })
    }

    /// Bracket pair colorization: 3 cycling colors + the unmatched-bracket color.
    pub fn bracket_colors(&self) -> crate::highlight::BracketColors {
        let c = |k: &str, d: &str| {
            let c = self.color_or(k, d);
            (c.r, c.g, c.b, c.a)
        };
        crate::highlight::BracketColors {
            cycle: [
                c("editorBracketHighlight.foreground1", "#FFD700"),
                c("editorBracketHighlight.foreground2", "#DA70D6"),
                c("editorBracketHighlight.foreground3", "#179FFF"),
            ],
            unexpected: c("editorBracketHighlight.unexpectedBracket.foreground", "#FF1212CC"),
        }
    }

    /// (foreground, background, 16-color palette) for VTE.
    pub fn terminal_colors(&self) -> (Rgba, Rgba, Vec<Rgba>) {
        const ANSI: [&str; 16] = [
            "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
            "BrightBlack", "BrightRed", "BrightGreen", "BrightYellow", "BrightBlue",
            "BrightMagenta", "BrightCyan", "BrightWhite",
        ];
        let fg = self.color_or("terminal.foreground", "#CCCCCC");
        let bg = self.color("terminal.background").unwrap_or_else(|| self.fallback_for("terminal.background"));
        let palette = ANSI.iter().map(|n| self.color_or(&format!("terminal.ansi{n}"), "#CCCCCC")).collect();
        (fg, bg, palette)
    }
}

pub fn user_theme_dir() -> PathBuf {
    crate::config_dir().join("themes")
}

fn read_theme_source(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(_) => {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            BUILTIN
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, src)| src.to_string())
                .ok_or_else(|| format!("theme not found: {}", path.display()))
        }
    }
}

fn parse_font_style(s: &str) -> FontStyle {
    let mut fs = FontStyle::empty();
    for part in s.split_whitespace() {
        match part {
            "bold" => fs |= FontStyle::BOLD,
            "italic" => fs |= FontStyle::ITALIC,
            "underline" => fs |= FontStyle::UNDERLINE,
            _ => {}
        }
    }
    fs
}

/// Strip `//` and `/* */` comments and trailing commas so JSONC parses as JSON.
pub fn strip_jsonc(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            out.push(c);
            if c == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1]);
                i += 1;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
            out.push(b'"');
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        } else if c == b',' {
            let next = bytes[i + 1..].iter().find(|b| !b.is_ascii_whitespace());
            if !matches!(next, Some(b'}') | Some(b']')) {
                out.push(b',');
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc() {
        let s = "{ // c\n \"a\": \"x//y\", /* b */ \"b\": [1,2,], \"é\": 1, }";
        let v: Value = serde_json::from_str(&strip_jsonc(s)).unwrap();
        assert_eq!(v["a"], "x//y");
        assert_eq!(v["b"].as_array().unwrap().len(), 2);
        assert_eq!(v["é"], 1);
    }

    #[test]
    fn default_theme_chain_resolves() {
        let t = ThemeModel::load(DEFAULT_THEME).unwrap();
        assert_eq!(t.color("editor.background"), Rgba::parse("#121314"));
        // keyword color comes from dark_plus/dark_vs via the include chain
        assert!(t.to_syntect().scopes.len() > 50);
    }

    #[test]
    fn colors() {
        assert_eq!(Rgba::parse("#FFFFFF22"), Some(Rgba { r: 255, g: 255, b: 255, a: 0x22 }));
        assert_eq!(Rgba::parse("#abc"), Some(Rgba { r: 0xaa, g: 0xbb, b: 0xcc, a: 255 }));
    }
}

//! Color themes.
//!
//! Three sources, all VS Code color-theme JSON:
//! - VS Code's own default themes, bundled (full workbench colors, `include` chains),
//! - user themes dropped into `~/.config/vibey/themes/` (JSONC allowed),
//! - the 60+ community themes embedded in irosashi (One Dark Pro, Dracula, …).
//!
//! A `ThemeModel` provides workbench colors (→ GTK CSS, VTE palette); token colors are
//! resolved by irosashi, which gets JSON-sourced themes registered under the theme's id.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::highlight;

/// VS Code's built-in themes: (id, label, file).
const VSCODE_THEMES: &[(&str, &str, &str)] = &[
    ("dark-2026", "Dark 2026", "2026-dark.json"),
    ("dark-modern", "Dark Modern", "dark_modern.json"),
    ("dark-plus-vscode", "Dark+", "dark_plus.json"),
    ("light-2026", "Light 2026", "2026-light.json"),
    ("light-modern", "Light Modern", "light_modern.json"),
    ("light-plus-vscode", "Light+", "light_plus.json"),
];

const BUILTIN_FILES: &[(&str, &str)] = &[
    ("2026-dark.json", include_str!("../assets/themes/2026-dark.json")),
    ("dark_modern.json", include_str!("../assets/themes/dark_modern.json")),
    ("dark_plus.json", include_str!("../assets/themes/dark_plus.json")),
    ("dark_vs.json", include_str!("../assets/themes/dark_vs.json")),
    ("2026-light.json", include_str!("../assets/themes/2026-light.json")),
    ("light_modern.json", include_str!("../assets/themes/light_modern.json")),
    ("light_plus.json", include_str!("../assets/themes/light_plus.json")),
    ("light_vs.json", include_str!("../assets/themes/light_vs.json")),
];

/// irosashi's trimmed copies of themes we bundle in full.
const SKIP_BUNDLED: &[&str] = &["dark-plus", "light-plus"];

pub const DEFAULT_THEME: &str = "dark-2026";

/// VS Code's built-in defaults for keys that themes commonly leave unset.
const DARK_DEFAULTS: &[(&str, &str)] = &[
    ("editor.background", "#1E1E1E"),
    ("editor.foreground", "#D4D4D4"),
    ("foreground", "#CCCCCC"),
    ("focusBorder", "#007FD4"),
    ("editorLineNumber.foreground", "#858585"),
    ("editorLineNumber.activeForeground", "#C6C6C6"),
    ("editor.selectionBackground", "#264F78"),
    ("editor.findMatchBackground", "#515C6A"),
    ("editor.findMatchHighlightBackground", "#EA5C0055"),
    ("diffEditor.insertedLineBackground", "#9BB95533"),
    ("list.highlightForeground", "#2AAAFF"),
    ("list.activeSelectionBackground", "#04395E"),
    ("list.inactiveSelectionBackground", "#37373D"),
    ("list.hoverBackground", "#2A2D2E"),
    ("quickInput.background", "#252526"),
    ("quickInputList.focusBackground", "#04395E"),
    ("widget.shadow", "#0000005C"),
    ("input.background", "#3C3C3C"),
    ("input.foreground", "#CCCCCC"),
    ("input.border", "#00000000"),
    ("inputOption.activeBorder", "#007ACC"),
    ("inputOption.activeBackground", "#007FD466"),
    ("tree.indentGuidesStroke", "#585858"),
    ("scrollbarSlider.background", "#79797966"),
    ("scrollbarSlider.hoverBackground", "#646464B3"),
    ("scrollbarSlider.activeBackground", "#BFBFBF66"),
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
    ("gitDecoration.modifiedResourceForeground", "#E2C08D"),
    ("gitDecoration.untrackedResourceForeground", "#73C991"),
    ("gitDecoration.addedResourceForeground", "#81B88B"),
    ("gitDecoration.deletedResourceForeground", "#C74E39"),
    ("gitDecoration.ignoredResourceForeground", "#8C8C8C"),
    ("editorGutter.addedBackground", "#2EA043"),
    ("editorGutter.modifiedBackground", "#0078D4"),
    ("editorGutter.deletedBackground", "#F85149"),
    ("editorBracketHighlight.foreground1", "#FFD700"),
    ("editorBracketHighlight.foreground2", "#DA70D6"),
    ("editorBracketHighlight.foreground3", "#179FFF"),
    ("editorBracketHighlight.unexpectedBracket.foreground", "#FF1212CC"),
];

const LIGHT_DEFAULTS: &[(&str, &str)] = &[
    ("editor.background", "#FFFFFF"),
    ("editor.foreground", "#000000"),
    ("foreground", "#616161"),
    ("focusBorder", "#0090F1"),
    ("editorLineNumber.foreground", "#237893"),
    ("editorLineNumber.activeForeground", "#0B216F"),
    ("editor.selectionBackground", "#ADD6FF"),
    ("editor.findMatchBackground", "#A8AC94"),
    ("editor.findMatchHighlightBackground", "#EA5C0055"),
    ("diffEditor.insertedLineBackground", "#9BB95533"),
    ("list.highlightForeground", "#0066BF"),
    ("list.activeSelectionBackground", "#0060C0"),
    ("list.inactiveSelectionBackground", "#E4E6F1"),
    ("list.hoverBackground", "#E8E8E8"),
    ("quickInput.background", "#F3F3F3"),
    ("quickInputList.focusBackground", "#D6EBFF"),
    ("widget.shadow", "#00000029"),
    ("input.background", "#FFFFFF"),
    ("input.foreground", "#616161"),
    ("input.border", "#CECECE"),
    ("inputOption.activeBorder", "#007ACC"),
    ("inputOption.activeBackground", "#0090F133"),
    ("tree.indentGuidesStroke", "#A9A9A9"),
    ("scrollbarSlider.background", "#64646466"),
    ("scrollbarSlider.hoverBackground", "#646464B3"),
    ("scrollbarSlider.activeBackground", "#00000099"),
    ("terminal.foreground", "#333333"),
    ("terminal.ansiBlack", "#000000"),
    ("terminal.ansiRed", "#CD3131"),
    ("terminal.ansiGreen", "#107C10"),
    ("terminal.ansiYellow", "#949800"),
    ("terminal.ansiBlue", "#0451A5"),
    ("terminal.ansiMagenta", "#BC05BC"),
    ("terminal.ansiCyan", "#0598BC"),
    ("terminal.ansiWhite", "#555555"),
    ("terminal.ansiBrightBlack", "#666666"),
    ("terminal.ansiBrightRed", "#CD3131"),
    ("terminal.ansiBrightGreen", "#14CE14"),
    ("terminal.ansiBrightYellow", "#B5BA00"),
    ("terminal.ansiBrightBlue", "#0451A5"),
    ("terminal.ansiBrightMagenta", "#BC05BC"),
    ("terminal.ansiBrightCyan", "#0598BC"),
    ("terminal.ansiBrightWhite", "#A5A5A5"),
    ("terminalCursor.foreground", "#000000"),
    ("gitDecoration.modifiedResourceForeground", "#895503"),
    ("gitDecoration.untrackedResourceForeground", "#007100"),
    ("gitDecoration.addedResourceForeground", "#587C0C"),
    ("gitDecoration.deletedResourceForeground", "#AD0707"),
    ("gitDecoration.ignoredResourceForeground", "#8E8E90"),
    ("editorGutter.addedBackground", "#2EA043"),
    ("editorGutter.modifiedBackground", "#005FB8"),
    ("editorGutter.deletedBackground", "#F85149"),
    ("editorBracketHighlight.foreground1", "#0431FA"),
    ("editorBracketHighlight.foreground2", "#319331"),
    ("editorBracketHighlight.foreground3", "#7B3814"),
    ("editorBracketHighlight.unexpectedBracket.foreground", "#FF1212CC"),
];

/// Workbench color keys exported to CSS as `@vs_<key with . → _>`.
const CSS_KEYS: &[&str] = &[
    "foreground",
    "focusBorder",
    "descriptionForeground",
    "widget.shadow",
    "editor.background",
    "editor.foreground",
    "editor.selectionBackground",
    "editorLineNumber.foreground",
    "editorGroupHeader.tabsBackground",
    "editorGroupHeader.tabsBorder",
    "editorWidget.background",
    "editorWidget.border",
    "tab.activeBackground",
    "tab.inactiveBackground",
    "tab.activeForeground",
    "tab.inactiveForeground",
    "tab.hoverBackground",
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
    "list.highlightForeground",
    "panel.background",
    "panel.border",
    "panelTitle.activeForeground",
    "panelTitle.inactiveForeground",
    "terminal.background",
    "statusBar.background",
    "statusBar.foreground",
    "statusBar.border",
    "scrollbarSlider.background",
    "scrollbarSlider.hoverBackground",
    "scrollbarSlider.activeBackground",
    "quickInput.background",
    "quickInput.foreground",
    "quickInputList.focusBackground",
    "quickInputList.focusForeground",
    "input.background",
    "input.foreground",
    "input.border",
    "inputOption.activeBorder",
    "inputOption.activeBackground",
    "button.background",
    "button.foreground",
    "badge.background",
    "badge.foreground",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dark,
    Light,
}

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
        if !h.is_ascii() {
            return None;
        }
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

    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    pub fn gdk(&self) -> gtk::gdk::RGBA {
        gtk::gdk::RGBA::new(
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        )
    }

    fn luminance(&self) -> f32 {
        (0.299 * self.r as f32 + 0.587 * self.g as f32 + 0.114 * self.b as f32) / 255.0
    }
}

#[derive(Clone, Debug)]
pub enum Source {
    Builtin(&'static str),
    User(PathBuf),
    Bundled(String),
}

#[derive(Clone, Debug)]
pub struct ThemeEntry {
    pub id: String,
    pub label: String,
    pub group: &'static str,
    pub source: Source,
}

/// Every available theme: VS Code built-ins, user themes, then bundled community themes.
pub fn catalog() -> Vec<ThemeEntry> {
    let mut out: Vec<ThemeEntry> = VSCODE_THEMES
        .iter()
        .map(|(id, label, file)| ThemeEntry {
            id: id.to_string(),
            label: label.to_string(),
            group: "VS Code",
            source: Source::Builtin(file),
        })
        .collect();
    if let Ok(dir) = std::fs::read_dir(user_theme_dir()) {
        let mut user: Vec<ThemeEntry> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .map(|p| {
                let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                let label = read_json(&p)
                    .ok()
                    .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_else(|| stem.clone());
                ThemeEntry { id: format!("user:{stem}"), label, group: "Your themes", source: Source::User(p) }
            })
            .collect();
        user.sort_by_key(|e| e.label.to_lowercase());
        out.extend(user);
    }
    let mut bundled: Vec<ThemeEntry> = highlight::highlighter()
        .themes()
        .into_iter()
        .filter(|id| !SKIP_BUNDLED.contains(&id.as_str()))
        .map(|id| ThemeEntry { label: bundled_label(&id), group: "Community", source: Source::Bundled(id.clone()), id })
        .collect();
    bundled.sort_by_key(|e| e.label.to_lowercase());
    out.extend(bundled);
    out
}

fn bundled_label(id: &str) -> String {
    id.split('-')
        .map(|w| match w {
            "github" => "GitHub".to_string(),
            "84" => "'84".to_string(),
            w => {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct ThemeModel {
    pub id: String,
    pub kind: Kind,
    colors: HashMap<String, String>,
}

impl ThemeModel {
    /// Load a theme by catalog id; falls back to the default theme.
    pub fn load(id: &str) -> ThemeModel {
        let catalog = catalog();
        let loaded = catalog.iter().find(|e| e.id == id).and_then(|e| {
            Self::from_entry(e).map_err(|err| eprintln!("vibey: theme {id}: {err}")).ok()
        });
        loaded.unwrap_or_else(|| {
            let e = catalog.iter().find(|e| e.id == DEFAULT_THEME).expect("default theme");
            Self::from_entry(e).expect("built-in default theme")
        })
    }

    fn from_entry(e: &ThemeEntry) -> Result<ThemeModel, String> {
        match &e.source {
            Source::Builtin(file) => Self::from_json_chain(e, Path::new(file)),
            Source::User(path) => Self::from_json_chain(e, path),
            Source::Bundled(name) => {
                let tc = highlight::highlighter().theme_colors(name).map_err(|e| e.to_string())?;
                let mut colors: HashMap<String, String> = tc.colors.into_iter().collect();
                colors.entry("editor.background".into()).or_insert(tc.background);
                colors.entry("editor.foreground".into()).or_insert(tc.foreground);
                let kind = if tc.kind == "light" { Kind::Light } else { Kind::Dark };
                Ok(ThemeModel { id: e.id.clone(), kind, colors })
            }
        }
    }

    /// Resolve a JSON theme's include chain and register its token rules with irosashi.
    fn from_json_chain(e: &ThemeEntry, path: &Path) -> Result<ThemeModel, String> {
        let mut colors = HashMap::new();
        let mut rules = Vec::new();
        let mut kind = None;
        load_chain(path, 0, &mut colors, &mut rules, &mut kind)?;
        let kind = kind.unwrap_or_else(|| {
            let bg = colors.get("editor.background").and_then(|c| Rgba::parse(c));
            if bg.is_some_and(|c| c.luminance() > 0.5) { Kind::Light } else { Kind::Dark }
        });
        let mut theme = Map::new();
        theme.insert("name".into(), Value::String(e.id.clone()));
        theme.insert("type".into(), Value::String(if kind == Kind::Light { "light" } else { "dark" }.into()));
        theme.insert(
            "colors".into(),
            Value::Object(colors.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect()),
        );
        theme.insert("tokenColors".into(), Value::Array(dedupe_rules(&rules)));
        highlight::register_theme(&e.id, Value::Object(theme).to_string().into_bytes())?;
        Ok(ThemeModel { id: e.id.clone(), kind, colors })
    }

    fn defaults(&self) -> &'static [(&'static str, &'static str)] {
        if self.kind == Kind::Light { LIGHT_DEFAULTS } else { DARK_DEFAULTS }
    }

    pub fn color(&self, key: &str) -> Option<Rgba> {
        self.colors
            .get(key)
            .and_then(|c| Rgba::parse(c))
            .or_else(|| self.defaults().iter().find(|(k, _)| *k == key).and_then(|(_, v)| Rgba::parse(v)))
    }

    /// A color, falling back the way VS Code derives unset keys from others.
    pub fn get(&self, key: &str) -> Rgba {
        if let Some(c) = self.color(key) {
            return c;
        }
        let from = match key {
            "terminal.background" => "panel.background",
            "panel.background" | "sideBar.background" | "activityBar.background" | "statusBar.background"
            | "editorGroupHeader.tabsBackground" | "tab.inactiveBackground" | "tab.activeBackground"
            | "breadcrumb.background" | "editorWidget.background" | "tab.hoverBackground" => "editor.background",
            "sideBarSectionHeader.background" => "sideBar.background",
            "quickInput.foreground" | "quickInputList.focusForeground" | "list.activeSelectionForeground"
            | "sideBarSectionHeader.foreground" | "sideBarTitle.foreground" | "sideBar.foreground"
            | "tab.activeForeground" | "panelTitle.activeForeground" | "activityBar.foreground"
            | "statusBar.foreground" => "foreground",
            "tab.inactiveForeground" | "panelTitle.inactiveForeground" | "activityBar.inactiveForeground"
            | "breadcrumb.foreground" | "descriptionForeground" => "editorLineNumber.foreground",
            "sideBar.border" | "activityBar.border" | "panel.border" | "statusBar.border" | "tab.border"
            | "editorGroupHeader.tabsBorder" | "editorWidget.border" => "widget.shadow",
            "badge.background" | "activityBarBadge.background" | "activityBar.activeBorder" | "tab.activeBorderTop"
            | "list.focusOutline" | "button.background" => "focusBorder",
            "badge.foreground" | "activityBarBadge.foreground" | "button.foreground" => "editor.foreground",
            _ => "foreground",
        };
        if from == key {
            return Rgba { r: 0xcc, g: 0xcc, b: 0xcc, a: 0xff };
        }
        self.get(from)
    }

    /// `@define-color` lines for every workbench key the stylesheet uses.
    pub fn css_colors(&self) -> String {
        CSS_KEYS
            .iter()
            .map(|key| format!("@define-color vs_{} {};\n", key.replace('.', "_"), self.get(key).css()))
            .collect()
    }

    /// (foreground, background, 16-color palette) for VTE.
    pub fn terminal_colors(&self) -> (Rgba, Rgba, Vec<Rgba>) {
        const ANSI: [&str; 16] = [
            "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White", "BrightBlack", "BrightRed",
            "BrightGreen", "BrightYellow", "BrightBlue", "BrightMagenta", "BrightCyan", "BrightWhite",
        ];
        let palette = ANSI.iter().map(|n| self.get(&format!("terminal.ansi{n}"))).collect();
        (self.get("terminal.foreground"), self.get("terminal.background"), palette)
    }

    /// Bracket pair colorization: 3 cycling colors + the unmatched-bracket color.
    pub fn bracket_colors(&self) -> highlight::BracketColors {
        let c = |k: &str| {
            let c = self.get(k);
            (c.r, c.g, c.b, c.a)
        };
        highlight::BracketColors {
            cycle: [
                c("editorBracketHighlight.foreground1"),
                c("editorBracketHighlight.foreground2"),
                c("editorBracketHighlight.foreground3"),
            ],
            unexpected: c("editorBracketHighlight.unexpectedBracket.foreground"),
        }
    }
}

fn load_chain(
    path: &Path,
    depth: u32,
    colors: &mut HashMap<String, String>,
    rules: &mut Vec<Value>,
    kind: &mut Option<Kind>,
) -> Result<(), String> {
    if depth > 8 {
        return Err("theme include chain too deep".into());
    }
    let json = read_json(path)?;
    if let Some(inc) = json.get("include").and_then(Value::as_str) {
        let inc = inc.trim_start_matches("./");
        load_chain(&path.parent().unwrap_or(Path::new("")).join(inc), depth + 1, colors, rules, kind)?;
    }
    match json.get("type").and_then(Value::as_str) {
        Some("light") | Some("hcLight") => *kind = Some(Kind::Light),
        Some("dark") | Some("hcDark") => *kind = Some(Kind::Dark),
        _ => {}
    }
    for (k, v) in json.get("colors").and_then(Value::as_object).into_iter().flatten() {
        if let Some(s) = v.as_str() {
            colors.insert(k.clone(), s.to_string());
        }
    }
    rules.extend(json.get("tokenColors").and_then(Value::as_array).into_iter().flatten().cloned());
    Ok(())
}

/// Built-in theme files are addressed by bare file name (so `include` resolves among them);
/// paths with a directory are read from disk, falling back to a built-in of the same name.
fn read_json(path: &Path) -> Result<Value, String> {
    let bare = path.parent().is_none_or(|p| p.as_os_str().is_empty());
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let builtin = || BUILTIN_FILES.iter().find(|(n, _)| *n == name).map(|(_, src)| src.to_string());
    let text = if bare {
        builtin().ok_or_else(|| format!("unknown built-in theme {name}"))?
    } else {
        match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(err) => builtin().ok_or_else(|| format!("{}: {err}", path.display()))?,
        }
    };
    serde_json::from_str(&strip_jsonc(&text)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Merge rules with identical selectors so the *later* rule wins property by property,
/// as in vscode-textmate. (irosashi 0.3 lets the first of two identical selectors win,
/// which would turn Dark 2026's gray comments green.)
pub fn dedupe_rules(rules: &[Value]) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut merged: HashMap<String, Map<String, Value>> = HashMap::new();
    for rule in rules {
        let Some(settings) = rule.get("settings").and_then(Value::as_object) else { continue };
        let selectors: Vec<String> = match rule.get("scope") {
            Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_string()).collect(),
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).collect(),
            _ => continue,
        };
        for sel in selectors.into_iter().filter(|s| !s.is_empty()) {
            let entry = merged.entry(sel.clone()).or_insert_with(|| {
                order.push(sel.clone());
                Map::new()
            });
            for key in ["foreground", "background", "fontStyle"] {
                if let Some(v) = settings.get(key) {
                    entry.insert(key.into(), v.clone());
                }
            }
        }
    }
    order
        .into_iter()
        .map(|sel| {
            let settings = merged.remove(&sel).unwrap_or_default();
            let mut rule = Map::new();
            rule.insert("scope".into(), Value::String(sel));
            rule.insert("settings".into(), Value::Object(settings));
            Value::Object(rule)
        })
        .collect()
}

pub fn user_theme_dir() -> PathBuf {
    crate::config_dir().join("themes")
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
        let t = ThemeModel::load(DEFAULT_THEME);
        assert_eq!(t.id, DEFAULT_THEME);
        assert_eq!(t.color("editor.background"), Rgba::parse("#121314"));
        assert_eq!(t.kind, Kind::Dark);
        assert_eq!(ThemeModel::load("light-2026").kind, Kind::Light);
    }

    #[test]
    fn later_rule_wins() {
        let rules: Vec<Value> = serde_json::from_str(
            r##"[{"scope":"comment","settings":{"foreground":"#00ff00","fontStyle":"italic"}},
                 {"scope":["comment","x.y"],"settings":{"foreground":"#888888"}}]"##,
        )
        .unwrap();
        let out = dedupe_rules(&rules);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["settings"]["foreground"], "#888888");
        assert_eq!(out[0]["settings"]["fontStyle"], "italic");
    }

    #[test]
    fn catalog_has_community_themes() {
        let c = catalog();
        assert!(c.iter().any(|e| e.label == "One Dark Pro"));
        assert!(!c.iter().any(|e| e.id == "dark-plus"));
        assert_eq!(ThemeModel::load("dracula").kind, Kind::Dark);
    }

    #[test]
    fn colors() {
        assert_eq!(Rgba::parse("#FFFFFF22"), Some(Rgba { r: 255, g: 255, b: 255, a: 0x22 }));
        assert_eq!(Rgba::parse("#abc"), Some(Rgba { r: 0xaa, g: 0xbb, b: 0xcc, a: 255 }));
    }
}

//! `~/.config/vibey/config.toml`.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Theme id from the theme picker (e.g. "dark-2026", "one-dark-pro", "user:mytheme").
    pub theme: String,
    /// "gpu" (GTK's GL renderer) or "software" (cairo: ~25 MB less memory, more CPU
    /// when scrolling). Takes effect on restart. `GSK_RENDERER` overrides it.
    pub renderer: String,
    /// Desktop notification when a terminal finishes while Vibey isn't focused.
    pub notifications: bool,
    /// Save an edited file when focus leaves it (another tab, a terminal, another window) or
    /// it's closed. Ctrl+S always saves.
    pub auto_save: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { theme: crate::theme::DEFAULT_THEME.into(), renderer: "gpu".into(), notifications: true, auto_save: true }
    }
}

fn path() -> std::path::PathBuf {
    crate::config_dir().join("config.toml")
}

pub fn load() -> Config {
    match std::fs::read_to_string(path()) {
        Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("vibey: config.toml: {e}");
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

/// Write atomically (temp file + rename) so a crash never leaves a half-written config.
pub fn save(cfg: &Config) {
    let p = path();
    let Ok(text) = toml::to_string_pretty(cfg) else { return };
    let _ = std::fs::create_dir_all(crate::config_dir());
    let tmp = p.with_extension("toml.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

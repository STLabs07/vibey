//! `~/.config/vibey/session.json`: open projects, their files and terminals, window size.
//! Terminals come back as fresh shells in their last working directory (processes can't
//! be restored).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Session {
    pub projects: Vec<ProjectState>,
    pub active: usize,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct ProjectState {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub active_file: Option<PathBuf>,
    /// Working directory of each terminal.
    pub terminals: Vec<PathBuf>,
}

fn path() -> PathBuf {
    crate::config_dir().join("session.json")
}

pub fn load() -> Session {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Write atomically (temp file + rename).
pub fn save(s: &Session) {
    let Ok(text) = serde_json::to_string_pretty(s) else { return };
    let _ = std::fs::create_dir_all(crate::config_dir());
    let p = path();
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_fields() {
        let s = Session {
            projects: vec![ProjectState {
                root: "/a".into(),
                files: vec!["/a/x.rs".into()],
                active_file: Some("/a/x.rs".into()),
                terminals: vec!["/a".into(), "/a/sub".into()],
            }],
            active: 0,
            width: 1400,
            height: 900,
            maximized: false,
        };
        let back: Session = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.projects[0].terminals.len(), 2);
        let partial: Session = serde_json::from_str(r#"{"projects":[{"root":"/b"}]}"#).unwrap();
        assert_eq!(partial.projects[0].root, PathBuf::from("/b"));
        assert!(partial.projects[0].files.is_empty());
    }
}

//! Git status via the system `git` (never libgit2), run off the UI thread.
//! Always `--no-optional-locks`: a background status must never hold `.git/index.lock`
//! while the AI CLI commits.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    // Ordered by how loudly a folder should show it.
    Ignored,
    Untracked,
    Added,
    Renamed,
    Modified,
    Deleted,
    Conflict,
}

impl Kind {
    pub fn letter(self) -> &'static str {
        match self {
            Kind::Ignored => "",
            Kind::Untracked => "U",
            Kind::Added => "A",
            Kind::Renamed => "R",
            Kind::Modified => "M",
            Kind::Deleted => "D",
            Kind::Conflict => "!",
        }
    }

    pub fn css(self) -> &'static str {
        match self {
            Kind::Ignored => "git-ignored",
            Kind::Untracked => "git-untracked",
            Kind::Added | Kind::Renamed => "git-added",
            Kind::Modified => "git-modified",
            Kind::Deleted => "git-deleted",
            Kind::Conflict => "git-conflict",
        }
    }

    pub const ALL_CSS: [&'static str; 6] =
        ["git-ignored", "git-untracked", "git-added", "git-modified", "git-deleted", "git-conflict"];
}

#[derive(Default, Debug)]
pub struct Status {
    pub toplevel: PathBuf,
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    files: HashMap<PathBuf, Kind>,
    dirs: HashMap<PathBuf, Kind>,
    ignored: HashSet<PathBuf>,
}

impl Status {
    pub fn is_repo(&self) -> bool {
        !self.toplevel.as_os_str().is_empty()
    }

    /// Number of changed (non-ignored) files.
    pub fn changed(&self) -> usize {
        self.files.len()
    }

    /// Decoration for a path: its own status, else the loudest status inside a folder,
    /// else ignored if it (or an ancestor) is ignored.
    pub fn kind(&self, abs: &Path, is_dir: bool) -> Option<Kind> {
        if let Some(k) = self.files.get(abs) {
            return Some(*k);
        }
        if is_dir {
            if let Some(k) = self.dirs.get(abs) {
                return Some(*k);
            }
        }
        let mut p = Some(abs);
        while let Some(cur) = p {
            if cur == self.toplevel {
                break;
            }
            if self.ignored.contains(cur) {
                return Some(Kind::Ignored);
            }
            p = cur.parent();
        }
        None
    }
}

/// The repository containing `path`: walks up looking for `.git` (dir, or file for worktrees).
pub fn repo_root(path: &Path) -> Option<PathBuf> {
    let mut p = if path.is_dir() { Some(path) } else { path.parent() };
    while let Some(dir) = p {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        p = dir.parent();
    }
    None
}

/// The `.git` directory (resolving `gitdir:` files used by worktrees/submodules).
pub fn git_dir(toplevel: &Path) -> Option<PathBuf> {
    let dot = toplevel.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let rel = text.trim().strip_prefix("gitdir:")?.trim();
    Some(toplevel.join(rel))
}

fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Run `git status` for the repository containing `project`.
pub fn status(project: &Path) -> Status {
    let Some(top) = repo_root(project) else { return Status::default() };
    let Some(out) = git(&top, &["status", "--porcelain=v2", "--branch", "-z", "--ignored=matching", "--untracked-files=all"])
    else {
        return Status::default();
    };
    parse(&top, &out)
}

pub fn parse(top: &Path, out: &[u8]) -> Status {
    let mut st = Status { toplevel: top.to_path_buf(), ..Default::default() };
    let mut fields = out.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            if let Some(b) = h.strip_prefix("branch.head ") {
                st.branch = (b != "(detached)").then(|| b.to_string());
            } else if let Some(ab) = h.strip_prefix("branch.ab ") {
                for part in ab.split_whitespace() {
                    if let Some(n) = part.strip_prefix('+') {
                        st.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        st.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        let (kind, path) = match rec.as_bytes()[0] {
            b'?' => (Kind::Untracked, rec[2..].to_string()),
            b'!' => {
                st.ignored.insert(top.join(rec[2..].trim_end_matches('/')));
                continue;
            }
            b'u' => (Kind::Conflict, rec.splitn(11, ' ').nth(10).unwrap_or("").to_string()),
            b'1' => (xy_kind(&rec[2..4]), rec.splitn(9, ' ').nth(8).unwrap_or("").to_string()),
            b'2' => {
                let path = rec.splitn(10, ' ').nth(9).unwrap_or("").to_string();
                fields.next(); // original path of the rename
                (Kind::Renamed, path)
            }
            _ => continue,
        };
        if path.is_empty() {
            continue;
        }
        let abs = top.join(&path);
        let mut dir = abs.parent();
        while let Some(d) = dir {
            if !d.starts_with(top) {
                break;
            }
            let e = st.dirs.entry(d.to_path_buf()).or_insert(kind);
            *e = (*e).max(kind);
            if d == top {
                break;
            }
            dir = d.parent();
        }
        st.files.insert(abs, kind);
    }
    st
}

fn xy_kind(xy: &str) -> Kind {
    let (x, y) = (xy.as_bytes()[0], xy.as_bytes()[1]);
    if x == b'D' || y == b'D' {
        Kind::Deleted
    } else if x == b'M' || y == b'M' || x == b'T' || y == b'T' {
        Kind::Modified
    } else if x == b'A' {
        Kind::Added
    } else if x == b'R' || x == b'C' {
        Kind::Renamed
    } else {
        Kind::Modified
    }
}

/// The staged (index) version of a file, which VS Code's gutter compares against.
/// `None` if untracked or not in a repository.
pub fn index_content(file: &Path) -> Option<String> {
    let top = repo_root(file)?;
    let rel = file.strip_prefix(&top).ok()?;
    let out = git(&top, &["show", &format!(":{}", rel.to_string_lossy())])?;
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Modification time of the repository's index, to know when cached index content is stale.
pub fn index_stamp(file: &Path) -> Option<std::time::SystemTime> {
    let top = repo_root(file)?;
    std::fs::metadata(git_dir(&top)?.join("index")).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let top = Path::new("/r");
        let out = b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 a a src/app.rs\0\
1 A. N... 000000 100644 100644 0 a src/new.rs\0\
2 R. N... 100644 100644 100644 a a R100 src/moved.rs\0src/old.rs\0\
? notes.txt\0! node_modules/\0! .env\0u UU N... 100644 100644 100644 100644 a b c src/conflict.rs\0";
        let st = parse(top, out);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.kind(Path::new("/r/src/app.rs"), false), Some(Kind::Modified));
        assert_eq!(st.kind(Path::new("/r/src/new.rs"), false), Some(Kind::Added));
        assert_eq!(st.kind(Path::new("/r/src/moved.rs"), false), Some(Kind::Renamed));
        assert_eq!(st.kind(Path::new("/r/notes.txt"), false), Some(Kind::Untracked));
        assert_eq!(st.kind(Path::new("/r/node_modules/react/index.js"), false), Some(Kind::Ignored));
        assert_eq!(st.kind(Path::new("/r/.env"), false), Some(Kind::Ignored));
        // Folders take their loudest child.
        assert_eq!(st.kind(Path::new("/r/src"), true), Some(Kind::Conflict));
        assert_eq!(st.kind(Path::new("/r/README.md"), false), None);
        assert_eq!(st.changed(), 5);
    }

    #[test]
    fn this_repo_has_a_branch() {
        let st = status(Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(st.is_repo());
        assert!(st.branch.is_some());
        assert!(index_content(&Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).is_some());
    }
}

//! Project-wide text search (read-only). Uses `git grep` in repositories (fast, respects
//! .gitignore, includes untracked files); otherwise scans the project's file index.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

/// Stop after this many matching lines (the UI says "showing first N").
pub const MAX_HITS: usize = 2000;
const MAX_FILE_BYTES: u64 = 2 << 20;
const SNIPPET: usize = 160;

pub struct Hit {
    /// 1-based line number.
    pub line: u32,
    /// The line (trimmed, possibly shortened around the match).
    pub text: String,
    /// Byte ranges of matches within `text`.
    pub ranges: Vec<(usize, usize)>,
    /// Byte column of the first match in the original line.
    pub col: usize,
}

pub struct FileHits {
    pub rel: String,
    pub hits: Vec<Hit>,
}

pub struct Results {
    pub files: Vec<FileHits>,
    pub total: usize,
    pub truncated: bool,
}

/// Byte ranges of `needle` in `hay` (ASCII case-insensitive unless `case`).
fn find_all(hay: &str, needle: &str, case: bool) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let (h, n) = if case { (hay.to_string(), needle.to_string()) } else { (hay.to_ascii_lowercase(), needle.to_ascii_lowercase()) };
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = h[from..].find(&n) {
        out.push((from + i, from + i + n.len()));
        from += i + n.len().max(1);
    }
    out
}

/// Turn a raw matching line into a hit with a readable snippet.
fn make_hit(line_no: u32, raw: &str, query: &str, case: bool) -> Option<Hit> {
    let ranges = find_all(raw, query, case);
    let first = *ranges.first()?;
    // Trim indentation; if long, start a little before the first match.
    let indent = raw.len() - raw.trim_start().len();
    let mut start = indent;
    if first.0 > start + SNIPPET / 2 {
        start = first.0 - SNIPPET / 3;
        while !raw.is_char_boundary(start) {
            start -= 1;
        }
    }
    let mut end = (start + SNIPPET).min(raw.len());
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    let text = raw[start..end].trim_end().to_string();
    let ranges = ranges
        .into_iter()
        .filter(|(a, b)| *a >= start && *b <= start + text.len())
        .map(|(a, b)| (a - start, b - start))
        .collect();
    Some(Hit { line: line_no, text, ranges, col: first.0 })
}

pub fn search(root: &Path, query: &str, case: bool, index: &[String]) -> Results {
    let mut files: BTreeMap<String, Vec<Hit>> = BTreeMap::new();
    let mut total = 0;
    let mut truncated = false;
    let used_git = crate::git::repo_root(root).is_some() && git_grep(root, query, case, &mut files, &mut total, &mut truncated);
    if !used_git {
        scan(root, query, case, index, &mut files, &mut total, &mut truncated);
    }
    Results { files: files.into_iter().map(|(rel, hits)| FileHits { rel, hits }).collect(), total, truncated }
}

fn git_grep(
    root: &Path,
    query: &str,
    case: bool,
    files: &mut BTreeMap<String, Vec<Hit>>,
    total: &mut usize,
    truncated: &mut bool,
) -> bool {
    let mut cmd = Command::new("git");
    cmd.args(["--no-optional-locks", "grep", "-n", "-I", "--untracked", "--no-color", "--null", "-F", "--max-count=200"]);
    if !case {
        cmd.arg("-i");
    }
    cmd.arg("-e").arg(query).arg("--").arg(".");
    let Ok(mut child) = cmd.current_dir(root).stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).spawn()
    else {
        return false;
    };
    let Some(out) = child.stdout.take() else { return false };
    for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
        // path \0 line \0 text
        let mut parts = line.splitn(3, |b| *b == 0);
        let (Some(path), Some(no), Some(text)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let Some(no) = std::str::from_utf8(no).ok().and_then(|n| n.parse().ok()) else { continue };
        let text = String::from_utf8_lossy(text);
        if let Some(hit) = make_hit(no, &text, query, case) {
            files.entry(String::from_utf8_lossy(path).into_owned()).or_default().push(hit);
            *total += 1;
            if *total >= MAX_HITS {
                *truncated = true;
                let _ = child.kill();
                break;
            }
        }
    }
    let status = child.wait();
    // Exit 1 = no matches (still a successful search); anything else = fall back.
    *truncated || status.is_ok_and(|s| s.code().is_some_and(|c| c <= 1))
}

fn scan(
    root: &Path,
    query: &str,
    case: bool,
    index: &[String],
    files: &mut BTreeMap<String, Vec<Hit>>,
    total: &mut usize,
    truncated: &mut bool,
) {
    for rel in index {
        let path = root.join(rel);
        if std::fs::metadata(&path).map_or(true, |m| m.len() > MAX_FILE_BYTES) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if bytes[..bytes.len().min(8192)].contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            if let Some(hit) = make_hit(i as u32 + 1, line, query, case) {
                files.entry(rel.clone()).or_default().push(hit);
                *total += 1;
                if *total >= MAX_HITS {
                    *truncated = true;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_and_ranges() {
        let h = make_hit(3, "        let foo = Foo::new(foo);", "foo", false).unwrap();
        assert_eq!(h.text, "let foo = Foo::new(foo);");
        assert_eq!(h.ranges, vec![(4, 7), (10, 13), (19, 22)]);
        assert_eq!(h.col, 12);
        assert!(make_hit(1, "nothing here", "foo", false).is_none());
        assert_eq!(make_hit(1, "Foo foo", "foo", true).unwrap().ranges, vec![(4, 7)]);
    }

    #[test]
    fn searches_this_repo() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let r = search(root, "fn dedupe_rules", true, &[]);
        assert!(r.files.iter().any(|f| f.rel == "src/theme.rs"));
        // Built at runtime so this file itself doesn't contain the query.
        let none = search(root, &format!("{}{}", "zz_no_such", "_text_qq"), false, &[]);
        assert_eq!(none.total, 0);
    }
}

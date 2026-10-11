//! Fuzzy file-path matching for quick open.
//!
//! Space-separated terms must all match (`src main` finds `src/app/main.rs`). Each term is
//! matched as a subsequence, preferring the file name, word starts and consecutive runs,
//! like fzf. Matches inside the file name rank far above matches spread over the path.

/// Returned positions are byte offsets into the candidate.
pub struct Match {
    pub score: i32,
    pub positions: Vec<usize>,
}

const SCORE_CHAR: i32 = 16;
const BONUS_BOUNDARY: i32 = 10;
const BONUS_CONSECUTIVE: i32 = 8;
const BONUS_FIRST_IN_NAME: i32 = 12;
const BONUS_ALL_IN_NAME: i32 = 60;
const BONUS_NAME_PREFIX: i32 = 40;
const BONUS_EXACT_NAME: i32 = 80;
const PENALTY_GAP: i32 = 1;

pub fn score(query: &str, path: &str) -> Option<Match> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Some(Match { score: 0, positions: Vec::new() });
    }
    let lower = path.to_ascii_lowercase();
    let name_start = path.rfind('/').map_or(0, |i| i + 1);
    let mut total = 0;
    let mut positions = Vec::new();
    for term in terms {
        let t = term.to_ascii_lowercase();
        // Try the file name first, then the whole path; keep the better result.
        let in_name = match_term(&t, &lower, path, name_start);
        let in_path = if name_start > 0 { match_term(&t, &lower, path, 0) } else { None };
        let best = match (in_name, in_path) {
            (Some(a), Some(b)) => if a.score >= b.score { a } else { b },
            (a, b) => a.or(b)?,
        };
        total += best.score;
        positions.extend(best.positions);
    }
    // Shorter paths win ties.
    total -= (path.len() / 8) as i32;
    positions.sort_unstable();
    positions.dedup();
    Some(Match { score: total, positions })
}

fn is_boundary(path: &[u8], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = path[i - 1];
    matches!(prev, b'/' | b'_' | b'-' | b'.' | b' ') || (prev.is_ascii_lowercase() && path[i].is_ascii_uppercase())
}

/// Match `term` (lowercase) as a subsequence of `lower[from..]`: find the leftmost end,
/// then walk backwards to the tightest start (fzf v1), and score it.
fn match_term(term: &str, lower: &str, orig: &str, from: usize) -> Option<Match> {
    let t = term.as_bytes();
    let l = lower.as_bytes();
    let o = orig.as_bytes();
    let mut ti = 0;
    let mut end = None;
    for (i, &c) in l.iter().enumerate().skip(from) {
        if c == t[ti] {
            ti += 1;
            if ti == t.len() {
                end = Some(i);
                break;
            }
        }
    }
    let end = end?;
    let mut ti = t.len();
    let mut start = end;
    for i in (from..=end).rev() {
        if l[i] == t[ti - 1] {
            ti -= 1;
            if ti == 0 {
                start = i;
                break;
            }
        }
    }
    // Forward again from the tight start, collecting positions.
    let mut positions = Vec::with_capacity(t.len());
    let mut ti = 0;
    for i in start..=end {
        if ti < t.len() && l[i] == t[ti] {
            positions.push(i);
            ti += 1;
        }
    }

    let name_start = orig.rfind('/').map_or(0, |i| i + 1);
    let mut score = 0;
    let mut prev: Option<usize> = None;
    for &p in &positions {
        score += SCORE_CHAR;
        if is_boundary(o, p) {
            score += BONUS_BOUNDARY;
        }
        match prev {
            Some(q) if q + 1 == p => score += BONUS_CONSECUTIVE,
            Some(q) => score -= PENALTY_GAP * (p - q - 1).min(20) as i32,
            None => {}
        }
        prev = Some(p);
    }
    if positions.first().is_some_and(|&p| p >= name_start) {
        score += BONUS_FIRST_IN_NAME + BONUS_ALL_IN_NAME;
        if positions[0] == name_start {
            score += BONUS_NAME_PREFIX;
            let name = &lower[name_start..];
            if name == term || name.split('.').next() == Some(term) {
                score += BONUS_EXACT_NAME;
            }
        }
    }
    Some(Match { score, positions })
}

/// Best `limit` matches, highest score first.
pub fn top<'a>(query: &str, paths: &'a [String], limit: usize) -> Vec<(&'a str, Match)> {
    let mut out: Vec<(&str, Match)> = paths.iter().filter_map(|p| score(query, p).map(|m| (p.as_str(), m))).collect();
    let cmp = |a: &(&str, Match), b: &(&str, Match)| b.1.score.cmp(&a.1.score).then_with(|| a.0.cmp(b.0));
    if out.len() > limit {
        out.select_nth_unstable_by(limit, cmp);
        out.truncate(limit);
    }
    out.sort_by(cmp);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank(q: &str, paths: &[&str]) -> Vec<String> {
        let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        top(q, &paths, 10).into_iter().map(|(p, _)| p.to_string()).collect()
    }

    #[test]
    fn prefers_file_name() {
        let r = rank("main", &["src/maintenance/lib.rs", "src/main.rs", "docs/m_a_i_n.md"]);
        assert_eq!(r[0], "src/main.rs");
    }

    #[test]
    fn multiple_terms() {
        let r = rank("app main", &["src/app/main.rs", "src/main.rs", "app/other.rs"]);
        assert_eq!(r, vec!["src/app/main.rs"]);
    }

    #[test]
    fn exact_name_beats_longer() {
        let r = rank("codeview", &["src/codeview_old_backup.rs", "src/codeview.rs"]);
        assert_eq!(r[0], "src/codeview.rs");
    }

    #[test]
    fn word_starts() {
        let r = rank("fmr", &["src/forms/mirror.rs", "src/file_manager_rust.rs"]);
        assert_eq!(r[0], "src/file_manager_rust.rs");
    }

    #[test]
    fn positions_point_into_name() {
        let m = score("main", "src/main.rs").unwrap();
        assert_eq!(m.positions, vec![4, 5, 6, 7]);
        assert!(score("xyz", "src/main.rs").is_none());
    }
}

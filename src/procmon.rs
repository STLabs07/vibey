//! Process/resource sampling straight from /proc (no dependencies).
//!
//! CPU% comes from utime+stime deltas between two samples (100% = one core, like `top`).
//! Memory is PSS from `smaps_rollup` (RSS double-counts shared pages), falling back to RSS.

use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Debug)]
struct Proc {
    ppid: i32,
    ticks: u64,
}

/// One process and its descendants, with usage.
#[derive(Clone, Debug)]
pub struct Node {
    pub pid: i32,
    pub name: String,
    pub cpu: f32,
    pub mem_kb: u64,
    pub children: Vec<Node>,
}

impl Node {
    pub fn total_cpu(&self) -> f32 {
        self.cpu + self.children.iter().map(Node::total_cpu).sum::<f32>()
    }

    pub fn total_mem(&self) -> u64 {
        self.mem_kb + self.children.iter().map(Node::total_mem).sum::<u64>()
    }
}

pub struct Report {
    /// One process tree per requested root pid (`None` if it's gone).
    pub roots: Vec<Option<Node>>,
    /// Vibey's own process (without its children).
    pub vibey: Node,
    /// Vibey + every descendant (shells, CLIs, dev servers…).
    pub total_cpu: f32,
    pub total_mem_kb: u64,
    pub load1: f32,
    pub cpus: usize,
    pub mem_total_kb: u64,
    pub mem_available_kb: u64,
}

/// CPU ticks from the previous sample, to compute rates.
#[derive(Clone)]
pub struct Prev {
    ticks: HashMap<i32, u64>,
    at: Instant,
}

impl Default for Prev {
    fn default() -> Self {
        Prev { ticks: HashMap::new(), at: Instant::now() }
    }
}

fn hz() -> f64 {
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) as f64 }
}

/// Parse `/proc/<pid>/stat` → (ppid, utime + stime).
fn parse_stat(stat: &str) -> Option<(i32, u64)> {
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ppid = f.get(1)?.parse().ok()?;
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    Some((ppid, utime + stime))
}

/// Direct children of `pid`, from `/proc/<pid>/task/*/children` (no full /proc scan).
fn children_of(pid: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return out };
    for t in tasks.flatten() {
        if let Ok(s) = std::fs::read_to_string(t.path().join("children")) {
            out.extend(s.split_whitespace().filter_map(|c| c.parse::<i32>().ok()));
        }
    }
    out
}

/// `root` and all its descendants.
fn descendants(root: i32) -> HashMap<i32, Proc> {
    let mut out = HashMap::new();
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        if out.contains_key(&pid) {
            continue;
        }
        if let Some((ppid, ticks)) = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().as_deref().and_then(parse_stat) {
            out.insert(pid, Proc { ppid, ticks });
            stack.extend(children_of(pid));
        }
    }
    out
}

pub fn mem_kb(pid: i32) -> u64 {
    if let Ok(s) = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")) {
        if let Some(v) = s.lines().find_map(|l| l.strip_prefix("Pss:")) {
            return v.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
        }
    }
    let page_kb = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64 / 1024;
    std::fs::read_to_string(format!("/proc/{pid}/statm"))
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * page_kb)
}

/// Display name: `claude`, `cargo`, or the script for interpreters (`node server.js` → server).
pub fn process_name(pid: i32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    let base = |s: &str| -> String {
        let b = s.rsplit('/').next().unwrap_or(s);
        b.strip_suffix(".exe")
            .or_else(|| b.strip_suffix(".js"))
            .or_else(|| b.strip_suffix(".mjs"))
            .or_else(|| b.strip_suffix(".py"))
            .unwrap_or(b)
            .to_string()
    };
    let interpreter = matches!(comm, "node" | "bun" | "deno" | "python" | "python3" | "ruby" | "perl" | "sh" | "bash");
    if interpreter {
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let args: Vec<String> = cmdline.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        let mut it = args.iter().skip(1);
        while let Some(a) = it.next() {
            if a == "-m" {
                return it.next().map(|m| m.to_string());
            }
            if a == "-c" || a == "-e" {
                break;
            }
            if !a.starts_with('-') && !a.is_empty() {
                return Some(base(a));
            }
        }
    }
    Some(base(comm))
}

fn cpu(prev: &Prev, pid: i32, ticks: u64, dt: f64) -> f32 {
    match prev.ticks.get(&pid) {
        Some(&p) if dt > 0.0 && ticks >= p => ((ticks - p) as f64 / hz() / dt * 100.0) as f32,
        _ => 0.0,
    }
}

fn build(pid: i32, procs: &HashMap<i32, Proc>, kids: &HashMap<i32, Vec<i32>>, prev: &Prev, dt: f64) -> Option<Node> {
    let p = procs.get(&pid)?;
    let mut children: Vec<Node> =
        kids.get(&pid).into_iter().flatten().filter_map(|c| build(*c, procs, kids, prev, dt)).collect();
    children.sort_by_key(|c| c.pid);
    Some(Node {
        pid,
        name: process_name(pid).unwrap_or_else(|| pid.to_string()),
        cpu: cpu(prev, pid, p.ticks, dt),
        mem_kb: mem_kb(pid),
        children,
    })
}

/// Sample usage for the given root pids (terminal shells) plus Vibey itself. Only Vibey's
/// process tree is read (terminal shells are its children), never all of /proc.
pub fn sample(roots: &[i32], prev: &Prev) -> (Report, Prev) {
    let me = std::process::id() as i32;
    let mut procs = descendants(me);
    for r in roots {
        if !procs.contains_key(r) {
            procs.extend(descendants(*r));
        }
    }
    let now = Instant::now();
    let dt = (now - prev.at).as_secs_f64();
    let mut kids: HashMap<i32, Vec<i32>> = HashMap::new();
    for (pid, p) in &procs {
        kids.entry(p.ppid).or_default().push(*pid);
    }
    let vibey_tree = build(me, &procs, &kids, prev, dt);
    let (total_cpu, total_mem_kb) = vibey_tree.as_ref().map_or((0.0, 0), |t| (t.total_cpu(), t.total_mem()));
    let vibey = vibey_tree
        .map(|t| Node { children: Vec::new(), name: "Vibey".into(), ..t })
        .unwrap_or(Node { pid: me, name: "Vibey".into(), cpu: 0.0, mem_kb: mem_kb(me), children: Vec::new() });

    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |k: &str| -> u64 {
        meminfo.lines().find_map(|l| l.strip_prefix(k)).and_then(|v| v.split_whitespace().next()?.parse().ok()).unwrap_or(0)
    };
    let report = Report {
        roots: roots.iter().map(|r| build(*r, &procs, &kids, prev, dt)).collect(),
        vibey,
        total_cpu,
        total_mem_kb,
        load1: std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse().ok())
            .unwrap_or(0.0),
        cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
        mem_total_kb: field("MemTotal:"),
        mem_available_kb: field("MemAvailable:"),
    };
    let next = Prev { ticks: procs.into_iter().map(|(pid, p)| (pid, p.ticks)).collect(), at: now };
    (report, next)
}

/// `1.2 GB`, `340 MB`.
pub fn fmt_mem(kb: u64) -> String {
    let mb = kb as f64 / 1024.0;
    if mb >= 1000.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }
}

/// Compact form for the rail: `1.2G`, `340M`.
pub fn fmt_mem_short(kb: u64) -> String {
    let mb = kb as f64 / 1024.0;
    if mb >= 1000.0 { format!("{:.1}G", mb / 1024.0) } else { format!("{mb:.0}M") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_with_spaces_in_comm() {
        let stat = "1234 (my (weird) proc) S 1 1234 1234 0 -1 4194304 100 0 0 0 250 50 0 0 20 0 1 0";
        assert_eq!(parse_stat(stat), Some((1, 300)));
    }

    #[test]
    fn samples_own_process_tree() {
        let child = std::process::Command::new("sleep").arg("2").spawn().unwrap();
        let (r1, prev) = sample(&[child.id() as i32], &Prev::default());
        assert!(r1.vibey.mem_kb > 0);
        assert_eq!(r1.roots[0].as_ref().map(|n| n.name.as_str()), Some("sleep"));
        assert!(r1.total_mem_kb >= r1.vibey.mem_kb);
        let (r2, _) = sample(&[999_999_999], &prev);
        assert!(r2.roots[0].is_none());
        assert!(r2.cpus >= 1 && r2.mem_total_kb > 0);
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_mem(350 * 1024), "350 MB");
        assert_eq!(fmt_mem(1229 * 1024), "1.2 GB");
        assert_eq!(fmt_mem_short(1229 * 1024), "1.2G");
    }
}

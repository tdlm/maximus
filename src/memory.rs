use std::{collections::HashMap, process::Command};

/// Resident memory, in bytes, of maximus and the process trees it started.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    /// The maximus process alone; its sessions and shells are counted separately.
    pub maximus: u64,
    /// Each live claude session (label, bytes), including what claude started: tools, MCP
    /// servers, hooks.
    pub sessions: Vec<(String, u64)>,
    /// Each terminal shell (label, bytes), including whatever runs in it.
    pub shells: Vec<(String, u64)>,
}

impl Usage {
    pub fn sessions_total(&self) -> u64 {
        self.sessions.iter().map(|(_, b)| b).sum()
    }

    pub fn shells_total(&self) -> u64 {
        self.shells.iter().map(|(_, b)| b).sum()
    }

    pub fn total(&self) -> u64 {
        self.maximus + self.sessions_total() + self.shells_total()
    }
}

/// One `ps` row: pid, parent pid, resident size in KiB.
type Proc = (u32, u32, u64);

fn parse_ps(out: &str) -> Vec<Proc> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace().map(str::parse::<u64>);
            let (pid, ppid, rss) = (f.next()?.ok()?, f.next()?.ok()?, f.next()?.ok()?);
            Some((pid as u32, ppid as u32, rss))
        })
        .collect()
}

/// Bytes resident in `root` and all its descendants.
fn tree_bytes(rss: &HashMap<u32, u64>, children: &HashMap<u32, Vec<u32>>, root: u32) -> u64 {
    let mut kib = 0;
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        kib += rss.get(&p).copied().unwrap_or(0);
        if let Some(c) = children.get(&p) {
            stack.extend(c);
        }
    }
    kib * 1024
}

fn usage_of(
    procs: &[Proc],
    own: u32,
    sessions: Vec<(String, u32)>,
    shells: Vec<(String, u32)>,
) -> Usage {
    let rss: HashMap<u32, u64> = procs.iter().map(|&(p, _, r)| (p, r)).collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(pid, ppid, _) in procs {
        children.entry(ppid).or_default().push(pid);
    }
    let measure = |list: Vec<(String, u32)>| {
        let mut v: Vec<(String, u64)> = list
            .into_iter()
            .map(|(label, pid)| (label, tree_bytes(&rss, &children, pid)))
            .collect();
        v.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
        v
    };
    Usage {
        maximus: rss.get(&own).map_or(0, |r| r * 1024),
        sessions: measure(sessions),
        shells: measure(shells),
    }
}

/// Measures maximus and the given (label, pid) sessions and shells with one `ps` run.
pub fn measure(sessions: Vec<(String, u32)>, shells: Vec<(String, u32)>) -> Usage {
    let out = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,rss="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    usage_of(&parse_ps(&out), std::process::id(), sessions, shells)
}

/// Bytes as a short human size, e.g. `412 MB`, `1.3 GB`.
pub fn human(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_process_trees() {
        let procs = parse_ps(
            "    1     0   100\n\
             \x20  10     1  2000\n\
             \x20  20    10  1000\n\
             \x20  21    10   500\n\
             \x20  30    20    50\n\
             \x20  40    10  4000\n\
             \x20  41    40   300\n\
             garbage line\n",
        );
        let u = usage_of(
            &procs,
            10,
            vec![("a".into(), 20), ("b".into(), 21)],
            vec![("sh".into(), 40)],
        );
        assert_eq!(u.maximus, 2000 * 1024);
        assert_eq!(
            u.sessions,
            vec![("a".into(), 1050 * 1024), ("b".into(), 500 * 1024)]
        );
        assert_eq!(u.shells, vec![("sh".into(), 4300 * 1024)]);
        assert_eq!(u.total(), (2000 + 1550 + 4300) * 1024);
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(human(5 * 1024 * 1024 + 512 * 1024), "5.5 MB");
        assert_eq!(human(412 * 1024 * 1024), "412 MB");
        assert_eq!(human(1331 * 1024 * 1024), "1.3 GB");
    }
}

use std::collections::HashSet;

use procfs::process::Process;

pub struct ProcessTree {
    edges: Vec<(i32, i32)>,
}

impl ProcessTree {
    pub fn new(all_processes: &[Process]) -> Self {
        let mut tree = ProcessTree { edges: Vec::new() };

        for p in all_processes.iter() {
            let pid = p.pid;
            let ppid = match p.status() {
                Ok(status) => status.ppid,
                Err(_) => continue,
            };

            tree.edges.push((ppid, pid));
        }

        tree
    }

    pub fn ancestors(&self, pid: i32, include_first: bool) -> Vec<i32> {
        let mut ancestors = if include_first { vec![pid] } else { Vec::new() };

        let mut current = pid;
        'outer: while current != 1 {
            for &(ppid, pid) in &self.edges {
                if pid == current {
                    ancestors.push(ppid);
                    current = ppid;
                    continue 'outer;
                }
            }

            // can't find parents up to 1, a process should have vanished in the meantime
            // this will be a partial tree
            return ancestors;
        }

        ancestors
    }

    pub fn descendants(&self, pid: i32) -> HashSet<i32> {
        let mut descendants: HashSet<i32> = HashSet::new();
        let mut pool: Vec<i32> = Vec::new();
        pool.push(pid);

        while let Some(current) = pool.pop() {
            for &(ppid, pid) in &self.edges {
                if ppid == current {
                    if !descendants.contains(&pid) {
                        descendants.insert(pid);
                        pool.push(pid);
                    }
                }
            }
        }

        descendants
    }
}

#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unreachable_code)]
#![allow(unused_imports)]
#![feature(drain_filter)]

// Process groups memory statistics tool
// - Must run as root
// - Don't forget to set a memory limit (-m/--memory-limit)
//
//
// TODO:
// - merge splitters
// - display: add swap
// - benchmark compile flags https://rust-lang.github.io/packed_simd/perf-guide/target-feature/rustflags.html
// - bench memory usage
// - filters
//   - remove &str[] / restrict to ascii
// - remove unwraps
// - custom hashset?

use clap::Parser;
use core::panic;
use indicatif::{ProgressBar, ProgressStyle};
use log::warn;
#[allow(unused_imports)]
use log::{debug, error, info, Level};
use procfs::{
    process::{PageInfo, Pfn, Process},
    PhysicalPageFlags,
};
use rayon::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    num::NonZeroUsize,
    os::unix::process::CommandExt,
    process::Command,
    sync::{Arc, Mutex},
};

use crate::{
    process_tree::ProcessTree,
    splitters::{
        ProcessSplitter, ProcessSplitterCustomFilter, ProcessSplitterEnvVariable,
        ProcessSplitterPids, ProcessSplitterUid,
    },
};

#[cfg(feature = "std")]
type ProcessGroupPfns = HashSet<Pfn>;
#[cfg(feature = "std")]
type ProcessInfoPfns = HashSet<Pfn>;

#[cfg(feature = "fnv")]
type ProcessGroupPfns = HashSet<Pfn, fnv::FnvBuildHasher>;
#[cfg(feature = "fnv")]
type ProcessInfoPfns = HashSet<Pfn, fnv::FnvBuildHasher>;

#[cfg(feature = "ahash")]
type ProcessGroupPfns = HashSet<Pfn, ahash::RandomState>;
#[cfg(feature = "ahash")]
type ProcessInfoPfns = HashSet<Pfn, ahash::RandomState>;

#[cfg(feature = "metrohash")]
type ProcessGroupPfns = HashSet<Pfn, metrohash::MetroBuildHasher>;
#[cfg(feature = "metrohash")]
type ProcessInfoPfns = HashSet<Pfn, metrohash::MetroBuildHasher>;

#[cfg(feature = "fxhash")]
type ProcessGroupPfns = rustc_hash::FxHashSet<Pfn>;
#[cfg(feature = "fxhash")]
type ProcessInfoPfns = rustc_hash::FxHashSet<Pfn>;

pub struct ProcessInfo {
    process: Process,
    uid: u32,
    environ: HashMap<OsString, OsString>,
    pfns: ProcessInfoPfns,
    swap_pages: HashSet<(u64, u64)>,
    rss: u64,
    vsz: u64,
    pte: u64,
    fds: usize,
}

pub struct ProcessGroupInfo {
    name: String,
    processes_info: Vec<ProcessInfo>,
    pfns: ProcessGroupPfns,
    swap_pages: HashSet<(u64, u64)>,
    pte: u64,
    fds: usize,
}

impl PartialEq for ProcessGroupInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

struct SmonInfo {
    pid: i32,
    sid: OsString,
    sga_size: u64,
    //sga_shm: Shm,
    //sga_pfns: HashSet<Pfn>,
}

// return info memory maps info for standard process or None for kernel process
fn get_info(process: Process) -> Result<ProcessInfo, Box<dyn std::error::Error>> {
    if process.cmdline()?.is_empty() {
        // already handled in main
        return Err(String::from("No info for kernel process"))?;
    }

    let page_size = procfs::page_size();

    // physical memory pages
    let mut pfns: ProcessInfoPfns = Default::default();
    // swap type, offset
    let mut swap_pages: HashSet<(u64, u64)> = HashSet::new();

    // size of pages in memory
    let mut rss = 0;
    // size of mappings
    let mut vsz = 0;

    // page table size
    let pte = process
        .status()?
        .vmpte
        .expect("'vmpte' field does not exist");

    // file descriptors
    let fds = process.fd_count()?;

    let memory_maps = snap::get_memory_maps_for_process(&process)?;

    for (memory_map, pages) in memory_maps.iter() {
        let size = memory_map.address.1 - memory_map.address.0;
        vsz += size;

        for page in pages.iter() {
            match page {
                PageInfo::MemoryPage(memory_page) => {
                    let pfn = memory_page.get_page_frame_number();
                    if pfn.0 != 0 {
                        rss += page_size;
                    }
                    pfns.insert(pfn);
                }
                PageInfo::SwapPage(swap_page) => {
                    let swap_type = swap_page.get_swap_type();
                    let offset = swap_page.get_swap_offset();

                    swap_pages.insert((swap_type, offset));
                }
            }
        }
    } // end for memory_maps

    let uid = process.uid()?;
    let env = process.environ()?;

    Ok(ProcessInfo {
        process,
        uid,
        environ: env,
        pfns,
        swap_pages,
        rss,
        vsz,
        pte,
        fds,
    })
}

fn processes_group_info(processes_info: Vec<ProcessInfo>, name: String) -> ProcessGroupInfo {
    let mut pfns: ProcessGroupPfns = HashSet::default();
    let mut swap_pages = HashSet::new();
    let mut pte = 0;
    let mut fds = 0;

    for process_info in &processes_info {
        pfns.par_extend(&process_info.pfns);
        swap_pages.par_extend(&process_info.swap_pages);
        pte += process_info.pte;
        fds += process_info.fds;
    }

    ProcessGroupInfo {
        name,
        processes_info,
        pfns,
        swap_pages,
        pte,
        fds,
    }
}

mod splitters {
    use std::{
        collections::{BTreeMap, HashMap, HashSet},
        ffi::{OsStr, OsString},
    };

    use anyhow::Context;
    use indicatif::ProgressBar;
    use itertools::Itertools;
    use log::{info, warn};
    use rayon::prelude::*;

    use crate::{
        filters::{self, Filter},
        process_tree::ProcessTree,
        processes_group_info, ProcessGroupInfo, ProcessGroupPfns, ProcessInfo,
    };

    pub trait ProcessSplitter<'a> {
        fn name(&self) -> String;
        type GroupIter<'b: 'a>: Iterator<Item = &'a ProcessGroupInfo>
        where
            Self: 'b;
        fn __split(&mut self, tree: &ProcessTree, processes: Vec<ProcessInfo>);
        fn iter_groups(&self) -> Self::GroupIter<'_>;
        fn collect_processes(self) -> Vec<ProcessInfo>;

        fn split(&mut self, tree: &ProcessTree, processes: Vec<ProcessInfo>) {
            let chrono = std::time::Instant::now();
            self.__split(tree, processes);
            info!("Split by {}: took {:?}", self.name(), chrono.elapsed());
        }

        fn display(&'a self) {
            let chrono = std::time::Instant::now();

            let mut info = Vec::new();
            let pb = ProgressBar::new(self.iter_groups().count() as u64);
            for group_1 in self.iter_groups() {
                let mut other_pfns: ProcessGroupPfns = HashSet::default();
                for group_2 in self.iter_groups() {
                    if group_1 != group_2 {
                        other_pfns.par_extend(&group_2.pfns);
                    }
                }

                let processes_count = group_1.processes_info.len();
                let rss = group_1.pfns.len() as u64 * procfs::page_size() / 1024 / 1024;
                let uss = group_1.pfns.difference(&other_pfns).count() as u64 * procfs::page_size()
                    / 1024
                    / 1024;

                info.push((group_1.name.clone(), processes_count, rss, uss));
                pb.inc(1);
            }
            pb.finish_and_clear();

            // sort by RSS
            info.sort_by(|a, b| b.2.cmp(&a.2));

            info!("Process groups by {}", self.name());
            info!("group_name                     #procs     RSS MiB     USS MiB",);
            info!("=============================================================");
            for (name, processes_count, rss, uss) in info {
                info!(
                    "{:<30}  {:>5}  {:>10}  {:>10}",
                    name, processes_count, rss, uss
                );
            }
            info!("Display split by {}: {:?}", self.name(), chrono.elapsed());
            info!("");
        }
    }

    pub struct ProcessSplitterCustomFilter {
        name: String,
        filters: Vec<Box<dyn Filter>>,
        names: Vec<String>,
        groups: HashMap<String, ProcessGroupInfo>,
    }
    impl ProcessSplitterCustomFilter {
        pub fn new(input: &str) -> Result<Self, Box<dyn std::error::Error>> {
            let mut filters: Vec<Box<dyn Filter>> = Vec::new();
            let mut names = Vec::new();
            let groups = HashMap::new();
            let mut counter = 0;

            loop {
                let (filter, ate) = filters::parse(&input[counter..])
                    .with_context(|| format!("Invalid filter {:?}", &input[counter..]))?;
                filters.push(filter);
                names.push(input[counter..(counter + ate)].to_string());
                counter += ate;
                if counter + 1 > input.chars().count() {
                    break;
                }
                counter += 1;
            }

            if counter < input.chars().count() {
                warn!("Didn't parse full filter {input:?}");
            }

            Ok(Self {
                name: input.to_string(),
                filters,
                names,
                groups,
            })
        }
    }
    impl<'a> ProcessSplitter<'a> for ProcessSplitterCustomFilter {
        fn name(&self) -> String {
            "Custom splitter".to_string()
        }

        type GroupIter<'b: 'a> = std::collections::hash_map::Values<'a, String, ProcessGroupInfo>;

        fn __split(&mut self, tree: &ProcessTree, mut processes: Vec<ProcessInfo>) {
            for (group_name, filter) in self.names.iter().zip(&self.filters) {
                let some_processes = processes
                    .drain_filter(|p| filter.eval(&p.process, tree))
                    .collect();
                let process_group_info = processes_group_info(some_processes, group_name.clone());
                self.groups.insert(group_name.clone(), process_group_info);
            }

            // remaining processes not captured by any filter
            let other_info = processes_group_info(processes, "Other".to_string());
            self.groups.insert("Other".to_string(), other_info);
        }

        fn iter_groups<'x>(&'a self) -> Self::GroupIter<'a> {
            self.groups.values()
        }

        fn collect_processes(mut self) -> Vec<ProcessInfo> {
            self.groups
                .par_drain()
                .flat_map(|(k, process_group_info)| process_group_info.processes_info)
                .collect()
        }
    }

    pub struct ProcessSplitterEnvVariable {
        var: OsString,
        groups: HashMap<Option<OsString>, ProcessGroupInfo>,
    }
    impl ProcessSplitterEnvVariable {
        pub fn new<S: AsRef<OsStr>>(var: S) -> Self {
            Self {
                groups: HashMap::new(),
                var: var.as_ref().to_os_string(),
            }
        }
    }

    impl<'a> ProcessSplitter<'a> for ProcessSplitterEnvVariable {
        type GroupIter<'b: 'a> =
            std::collections::hash_map::Values<'a, Option<OsString>, ProcessGroupInfo>;

        fn name(&self) -> String {
            format!("environment variable {}", self.var.to_string_lossy())
        }
        fn __split(&mut self, _tree: &ProcessTree, mut processes: Vec<ProcessInfo>) {
            let sids: HashSet<Option<OsString>> = processes
                .par_iter()
                .map(|p| p.environ.get(&self.var).cloned())
                .collect();

            let mut groups: HashMap<Option<OsString>, ProcessGroupInfo> = HashMap::new();
            for sid in sids {
                let some_processes: Vec<ProcessInfo> = processes
                    .drain_filter(|p| p.environ.get(&self.var) == sid.as_ref())
                    .collect();
                let name = format!(
                    "{:?}",
                    sid.as_ref().map(|os| os.to_string_lossy().to_string())
                );
                let process_group_info = processes_group_info(some_processes, name);
                groups.insert(sid, process_group_info);
            }
            self.groups = groups;
        }
        fn iter_groups<'x>(&'a self) -> Self::GroupIter<'a> {
            self.groups.values()
        }
        fn collect_processes(mut self) -> Vec<ProcessInfo> {
            self.groups
                .par_drain()
                .flat_map(|(k, process_group_info)| process_group_info.processes_info)
                .collect()
        }
    }
    pub struct ProcessSplitterPids {
        pids: Vec<i32>,
        groups: BTreeMap<u8, ProcessGroupInfo>,
    }

    impl ProcessSplitterPids {
        pub fn new(pids: &[i32]) -> Self {
            Self {
                pids: pids.to_vec(),
                groups: BTreeMap::new(),
            }
        }
    }
    impl<'a> ProcessSplitter<'a> for ProcessSplitterPids {
        type GroupIter<'b: 'a> = std::collections::btree_map::Values<'a, u8, ProcessGroupInfo>;

        fn name(&self) -> String {
            "PID list".to_string()
        }
        fn __split(&mut self, _tree: &ProcessTree, processes: Vec<ProcessInfo>) {
            let mut processes_info_0: Vec<ProcessInfo> = Vec::new();
            let mut processes_info_1: Vec<ProcessInfo> = Vec::new();

            for p in processes {
                if self.pids.contains(&p.process.pid) {
                    processes_info_0.push(p);
                } else {
                    processes_info_1.push(p);
                }
            }

            let name_0 = self.pids.iter().map(|pid| pid.to_string()).join(", ");
            let name_1 = "Others PIDs".into();
            let process_group_info_0 = processes_group_info(processes_info_0, name_0);
            let process_group_info_1 = processes_group_info(processes_info_1, name_1);

            self.groups.insert(0, process_group_info_0);
            self.groups.insert(1, process_group_info_1);
        }
        fn iter_groups<'x>(&'a self) -> Self::GroupIter<'a> {
            self.groups.values()
        }
        fn collect_processes(self) -> Vec<ProcessInfo> {
            self.groups
                .into_values()
                .flat_map(|group| group.processes_info)
                .collect()
        }
    }
    pub struct ProcessSplitterUid {
        groups: BTreeMap<u32, ProcessGroupInfo>,
    }

    impl ProcessSplitterUid {
        pub fn new() -> Self {
            Self {
                groups: BTreeMap::new(),
            }
        }
    }
    impl<'a> ProcessSplitter<'a> for ProcessSplitterUid {
        type GroupIter<'b: 'a> = std::collections::btree_map::Values<'a, u32, ProcessGroupInfo>;

        fn name(&self) -> String {
            "UID".to_string()
        }
        fn __split(&mut self, _tree: &ProcessTree, mut processes: Vec<ProcessInfo>) {
            let uids: HashSet<u32> = processes.iter().map(|p| p.uid).collect();

            for uid in uids {
                let username = users::get_user_by_uid(uid);
                let username = match username {
                    Some(username) => username.name().to_string_lossy().to_string(),
                    None => format!("{uid}"),
                };
                let processes_info: Vec<ProcessInfo> =
                    processes.drain_filter(|p| p.uid == uid).collect();
                let group_info = processes_group_info(processes_info, username);
                self.groups.insert(uid, group_info);
            }
        }
        fn iter_groups<'x>(&'a self) -> Self::GroupIter<'a> {
            self.groups.values()
        }
        fn collect_processes(self) -> Vec<ProcessInfo> {
            self.groups
                .into_values()
                .flat_map(|group| group.processes_info)
                .collect()
        }
    }
}

mod filters {
    use anyhow::{bail, Context, Result};
    use log::{debug, info, warn};
    use std::ffi::OsString;

    use procfs::process::Process;

    use crate::process_tree::ProcessTree;

    pub trait Filter: std::fmt::Debug {
        fn eval(&self, p: &Process, tree: &ProcessTree) -> bool;
    }

    #[derive(Debug)]
    struct NotFilter {
        inner: Box<dyn Filter>,
    }
    impl Filter for NotFilter {
        fn eval(&self, p: &Process, tree: &ProcessTree) -> bool {
            !self.inner.eval(p, tree)
        }
    }

    #[derive(Debug)]
    struct TrueFilter;
    impl Filter for TrueFilter {
        fn eval(&self, _p: &Process, _: &ProcessTree) -> bool {
            true
        }
    }

    #[derive(Debug)]
    struct FalseFilter;
    impl Filter for FalseFilter {
        fn eval(&self, _p: &Process, _: &ProcessTree) -> bool {
            false
        }
    }

    #[derive(Debug)]
    struct AndFilter {
        pub children: Vec<Box<dyn Filter>>,
    }
    impl Filter for AndFilter {
        fn eval(&self, p: &Process, tree: &ProcessTree) -> bool {
            self.children.iter().all(|child| child.eval(p, tree))
        }
    }

    #[derive(Debug)]
    struct OrFilter {
        pub children: Vec<Box<dyn Filter>>,
    }
    impl Filter for OrFilter {
        fn eval(&self, p: &Process, tree: &ProcessTree) -> bool {
            self.children.iter().any(|child| child.eval(p, tree))
        }
    }

    #[derive(Debug)]
    struct UidFilter {
        pub uid: u32,
    }
    impl Filter for UidFilter {
        fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
            self.uid == p.uid().unwrap()
        }
    }

    #[derive(Debug)]
    struct DescendantsFilter {
        pub pid: i32,
    }
    impl Filter for DescendantsFilter {
        fn eval(&self, p: &Process, tree: &ProcessTree) -> bool {
            let procs = tree.descendants(self.pid, true);
            procs.contains(&p.pid)
        }
    }

    #[derive(Debug)]
    struct PidFilter {
        pub pid: i32,
    }
    impl Filter for PidFilter {
        fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
            self.pid == p.pid
        }
    }

    #[derive(Debug)]
    pub struct EnvironKFilter {
        pub key: String,
    }
    impl Filter for EnvironKFilter {
        fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
            match p.environ() {
                Ok(e) => e.get(&OsString::from(&self.key)).is_some(),
                Err(_) => false,
            }
        }
    }

    #[derive(Debug)]
    pub struct EnvironKVFilter {
        pub key: String,
        pub value: String,
    }
    impl Filter for EnvironKVFilter {
        fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
            match p.environ() {
                Ok(e) => e.get(&OsString::from(&self.key)) == Some(&OsString::from(&self.value)),
                Err(_) => false,
            }
        }
    }

    /// uid(0)
    /// env(ORACLE_SID, PROD)
    /// or(uid(0), uid(1000))
    /// and(env(ORACLE_SID, PROD), uid(1000))
    /// descendant(1234)
    /// limitations:
    /// - vars can't contain ()
    pub fn parse(input: &str) -> Result<(Box<dyn Filter>, usize)> {
        debug!("Parsing: {input:?}");

        let opening = input
            .find('(')
            .with_context(|| "Missing opening parenthesis")?;

        let name: String = input.chars().take(opening).collect();
        debug!("operator: {:?}", name);

        fn find_match_par(input: &str, idx: usize) -> Result<usize> {
            let mut counter = 0;
            let mut iter = input.char_indices().skip(idx);
            loop {
                let Some((idx, c)) = iter.next() else {
                    bail!("Unbalanced parenthesis at end of string");
                };

                match c {
                    '(' => counter += 1,
                    ')' => counter -= 1,
                    _ => (),
                }

                if counter < 0 {
                    bail!("Too many closing parenthesis");
                }
                if counter == 0 {
                    return Ok(idx);
                }
            }
        }

        let closing = find_match_par(input, opening).unwrap();
        let inner: String = input
            .chars()
            .skip(opening + 1)
            .take(closing - opening - 1)
            .collect();
        debug!("opening, closing: {} {}", opening, closing);
        debug!("inner: {inner:?}");

        if closing + 1 != input.chars().count() {
            //warn!("Didn't consume whole filter");
            //info!("Parsing: {input:?}");
            //info!("opening, closing: {} {}", opening, closing);
            //info!("inner: {inner:?}");
        }

        let ate = closing + 1;
        debug!("ate: {ate:?}");

        match name.as_str() {
            "and" | "or" => {
                let mut from = 0;
                let mut inners = Vec::new();
                loop {
                    let Ok((parsed_inner, inner_ate)) = parse(&inner[from..]) else {
                        bail!("Can't parse {:?}", &inner[from..]);
                    };
                    inners.push(parsed_inner);
                    from += inner_ate + 1;

                    if from > inner.chars().count() {
                        break;
                    }
                }
                if inners.is_empty() {
                    bail!("Empty filter for {name:?}");
                }

                if name == "and" {
                    Ok((Box::new(AndFilter { children: inners }), ate))
                } else if name == "or" {
                    Ok((Box::new(OrFilter { children: inners }), ate))
                } else {
                    unreachable!()
                }
            }
            "not" => {
                let (parsed_inner, ate_2) = parse(&inner)?;
                if ate_2 < inner.chars().count() {
                    warn!("Ignored garbage {:?}", &inner[ate_2..]);
                }
                Ok((
                    Box::new(NotFilter {
                        inner: parsed_inner,
                    }),
                    ate,
                ))
            }
            "descendants" => {
                let inner = inner
                    .parse()
                    .with_context(|| "Argument of 'descendants' filter must be a number")?;
                Ok((Box::new(DescendantsFilter { pid: inner }), ate))
            }
            "pid" => {
                let inner = inner
                    .parse()
                    .with_context(|| "Argument of 'descendant' filter must be a number")?;
                Ok((Box::new(PidFilter { pid: inner }), ate))
            }
            "uid" => {
                let inner = inner
                    .parse()
                    .with_context(|| "Argument of 'descendant' filter must be a number")?;
                Ok((Box::new(UidFilter { uid: inner }), ate))
            }

            "env_kv" => {
                let mut iter = inner.split(',');
                let key = iter
                    .next()
                    .with_context(|| "Invalid key for env_kv")?
                    .trim()
                    .to_string();
                let value = iter
                    .next()
                    .with_context(|| "Invalid value for env_kv")?
                    .trim()
                    .to_string();
                Ok((Box::new(EnvironKVFilter { key, value }), ate))
            }
            "env_k" => {
                let key = inner;
                Ok((Box::new(EnvironKFilter { key }), ate))
            }
            "true" => Ok((Box::new(TrueFilter), ate)),
            "false" => Ok((Box::new(FalseFilter), ate)),
            x => bail!("Unknown filter: {x:?}"),
        }
    }
}

mod process_tree {
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

        pub fn descendants(&self, pid: i32, include_first: bool) -> HashSet<i32> {
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
}

/// Spawn new process with database user
/// return smon info
fn get_smon_info(
    pid: i32,
    uid: u32,
    sid: &OsStr,
    home: &OsStr,
) -> Result<SmonInfo, Box<dyn std::error::Error>> {
    let myself = std::env::args().next().unwrap();

    let mut lib = home.to_os_string();
    lib.push("/lib");

    let output = Command::new(myself)
        .env("LD_LIBRARY_PATH", lib)
        .env("ORACLE_SID", sid)
        .env("ORACLE_HOME", home)
        .uid(uid)
        .arg("--get-sga")
        .output()
        .expect("failed to execute process");

    if !output.status.success() {
        return Err(format!("Can't get info for {sid:?}: {:?}", output))?;
    }

    let stdout = match String::from_utf8(output.stdout.clone()) {
        Ok(s) => s,
        Err(_) => {
            return Err(format!("Can't read output for {sid:?}: {:?}", output))?;
        }
    };

    let sga_size: u64 = stdout.trim().parse().unwrap();

    // we can't be sure it's the correct shm
    //let (sga_shm, sga_pfns) = procfs::Shm::new()?
    //    .iter()
    //    .filter(|shm| shm.size as u64 == sga_size)
    //    .map(|shm| (shm.clone(), snap::shm2pfns(shm).unwrap()))
    //    .next()
    //    .unwrap();

    let result = SmonInfo {
        pid,
        //sga_pfns,
        //sga_shm,
        sga_size,
        sid: sid.to_os_string(),
    };

    Ok(result)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let global_chrono = std::time::Instant::now();

    const CLAP_ABOUT: &str = r"Scan processes, generates memory statistics for groups of processes";

    const AFTER_HELP: &str = r"/!\ Always set a memory limit /!\

Default limits:
    - memory: available memory / 2
    - threads: available CPU threads / 2

Available filters:
    - true()
    - false()
    - or(..)
    - and(..)
    - not(..)
    - uid(<uid>)
    - descendants(<pid>)
    - pid(<pid>)
    - env_k(<env key>)
    - env_kv(<env key, env value>)
Limitation:
    - ALL filters require trailing parenthesis
    - Spaces are not allowed before/after commas
Examples:
    - All processes for user 1000: uid(1000)
    - All processes that have a `DISPLAY` env variable (whatever its value is): env_k(DISPLAY)
    - All processes that have a `SHELL` env variable with value `/bin/bash`: env_kv(SHELL,/bin/bash)
    - All non-root processes that have a `DISPLAY` env variable: and(not(uid(0)),env_k(DISPLAY))";

    #[derive(Parser, Debug)]
    #[command(author, version, about, long_about = None, after_help = AFTER_HELP)]
    struct Cli {
        #[arg(long, hide(true))]
        get_sga: bool,

        #[arg(long, hide(true))]
        scan_oracle: bool,

        #[arg(long, hide(true))]
        scan_shm: bool,

        #[arg(long, hide(true))]
        scan_kpageflags: bool,

        #[arg(short, long)]
        mem_limit: Option<u64>,

        #[arg(short, long)]
        threads: Option<usize>,

        #[arg(short = 'e', long)]
        split_env: Option<String>,

        #[arg(short = 'u', long)]
        split_uid: bool,

        #[arg(short = 'p', long, action = clap::ArgAction::Append)]
        split_pids: Vec<i32>,

        #[arg(long, help = "Comma separated list of filters")]
        custom_split: Vec<String>,

        #[arg(short, long)]
        global_stats: bool,

        #[arg(short, long, help = "Filter to scan only a subset of processes")]
        filter: Option<String>,
    }

    let mut cli = Cli::parse();

    if cli.get_sga {
        // oracle shouldn't run as root
        assert_ne!(users::get_effective_uid(), 0);

        // subprogram to connect to instance and print sga size
        // We must have the correct context (user, env vars) to connect to database
        let sga_size = snap::get_sga_size().unwrap();

        // print value, can't use logger here
        // parent will grab that value in `get_smon_info`
        println!("{sga_size}");
        std::process::exit(0);
    }
    // can't print anything before that line

    //dbg!(&cli);
    if std::env::args().count() == 1 {
        use clap::CommandFactory;
        Cli::command().print_help().unwrap();
        std::process::exit(0);
    }

    let mem_limit = if let Some(m) = cli.mem_limit {
        m
    } else {
        let meminfo = procfs::Meminfo::new().unwrap();
        meminfo.mem_available.unwrap() / 1024 / 1024 / 2
    };
    info!("Memory limit: {mem_limit} MiB");
    let threads = if let Some(t) = cli.threads {
        t
    } else {
        std::thread::available_parallelism()
            .unwrap_or(NonZeroUsize::new(1).unwrap())
            .get()
            / 2
    };
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()
        .unwrap();

    info!("Using {threads} threads");
    info!("");

    // Main program starts here
    if users::get_effective_uid() != 0 {
        error!("Run as root");
        panic!();
    }

    let page_size = procfs::page_size();

    if !&cli.custom_split.is_empty() {
        // early parse filters
        for filter in &cli.custom_split {
            let _ = ProcessSplitterCustomFilter::new(filter).unwrap();
        }
    }

    if cli.scan_oracle {
        // find smons processes, and for each spawn a new process in the correct context to get database info
        info!("Scanning Oracle instances...");
        let instances: Vec<SmonInfo> = snap::find_smons()
            .iter()
            .filter_map(|(pid, uid, sid, home)| {
                let smon_info = get_smon_info(*pid, *uid, sid.as_os_str(), home.as_os_str());

                smon_info.ok()
            })
            .collect();

        if !instances.is_empty() {
            info!("Oracle instances:");
            info!("SID               SGA MiB");
            info!("==========================");
            for instance in &instances {
                info!(
                    "{:<12} {:>12}",
                    instance.sid.to_string_lossy(),
                    instance.sga_size / 1024 / 1024
                );
            }
            info!("");
        } else {
            warn!("Can't locate any Oracle instance");
        }
    }

    if cli.scan_shm {
        info!("Scanning shm...");
        let mut shms: HashMap<procfs::Shm, HashSet<Pfn>> = HashMap::new();
        for shm in procfs::Shm::new().expect("Can't read /dev/sysvipc/shm") {
            let pfns = snap::shm2pfns(&shm).unwrap();
            shms.insert(shm, pfns);
        }

        if !shms.is_empty() {
            info!("Shared memory segments:");
            info!("         key           id       PFNs    RSS MiB  % in RAM",);
            info!("==========================================================",);
            for (shm, pfns) in &shms {
                info!(
                    "{:>12} {:>12} {:>10} {:>10} {:>8.2}%",
                    shm.key,
                    shm.shmid,
                    pfns.len(),
                    pfns.len() * page_size as usize / 1024 / 1024,
                    (pfns.len() as u64 * page_size) as f32 / shm.size as f32 * 100.
                );
            }
            info!("");
        } else {
            warn!("Can't locate any shared memory segment")
        }
    }

    // probably incorrect?
    // size of kernel structures
    //let current_kernel = procfs::sys::kernel::Version::current().unwrap();
    //let (fd_size, task_size) =
    //    snap::get_kernel_datastructure_size(current_kernel).expect("Unknown kernel");

    //let mut kpagecount = procfs::KPageCount::new().expect("Can't open /proc/kpagecount");
    if cli.scan_kpageflags {
        info!("Scanning /proc/kpageflags...");
        let mut kpageflags = procfs::KPageFlags::new().expect("Can't open /proc/kpageflags");
        let all_physical_pages: HashMap<Pfn, PhysicalPageFlags> = procfs::iomem()
            .expect("Can't read iomem")
            .iter()
            .filter_map(|(_indent, map)| {
                if map.name == "System RAM" {
                    Some(map)
                } else {
                    None
                }
            })
            .map(|map| {
                let (start, end) = map.get_range();

                //let counts = kpagecount
                //    .get_count_in_range(start, end)
                //    .expect("Can't read /proc/kpagecount");
                let flags = kpageflags
                    .get_range_info(start, end)
                    .expect("Can't read /proc/kpagecount");
                let pfns: Vec<Pfn> = (start.0..end.0).map(Pfn).collect();

                use itertools::izip;
                let v: Vec<(Pfn, PhysicalPageFlags)> = izip!(pfns, flags).collect();

                v
            })
            .flatten()
            .collect();
    }

    let my_pid = std::process::id();
    let my_process = procfs::process::Process::new(my_pid as i32).unwrap();

    // processes are scanned once and reused to get a more consistent view
    let hit_memory_limit = Arc::new(Mutex::new(false));
    let mut kernel_processes_count = 0;
    let chrono = std::time::Instant::now();
    let all_processes: Vec<Process> = procfs::process::all_processes()
        .unwrap()
        .filter_map(|p| p.ok())
        .collect();
    let all_processes_count = all_processes.len();
    let tree = ProcessTree::new(&all_processes);

    let processes: Vec<Process> = if let Some(filter) = cli.filter {
        let (f, ate) = filters::parse(&filter).unwrap();
        if filter.chars().count() != ate {
            warn!("Ate {ate}, but filter is {} chars", filter.chars().count());
        }

        let processes: Vec<Process> = all_processes
            .into_iter()
            .filter(|p| f.eval(p, &tree))
            .collect();
        let processes_count = processes.len();

        if processes_count == 0 {
            warn!("Filter excluded all processes");
            warn!("filter: {filter:?}");
            return;
        }

        info!(
            "Filter excluded {} processes, {} processes remaining",
            all_processes_count - processes_count,
            processes_count
        );

        processes
    } else {
        all_processes
    };

    // exclude kernel procs
    let processes: Vec<Process> = processes
        .into_iter()
        .filter_map(|proc| {
            if proc.cmdline().ok()?.is_empty() {
                kernel_processes_count += 1;
                None
            } else {
                Some(proc)
            }
        })
        .collect();
    info!("{} kernel processes", kernel_processes_count);

    let processes_count = processes.len();

    info!("Scanning {processes_count} processes");
    let pb = ProgressBar::new(processes_count as u64);
    pb.set_style(ProgressStyle::with_template("{msg} {wide_bar} {pos}/{len}").unwrap());
    let processes_info: Vec<ProcessInfo> = processes
        .into_par_iter()
        //.progress_count(processes_count as u64)
        .filter_map(|proc| {
            let my_rss = my_process.status().unwrap().vmrss.unwrap() / 1024;
            pb.set_message(format!("{my_rss}/{mem_limit} MiB"));

            if my_rss > mem_limit {
                let mut guard = hit_memory_limit.lock().unwrap();
                if !*guard {
                    warn!(
                        "Hit memory limit ({} MiB), try increasing limit or filtering processes",
                        mem_limit
                    );
                    *guard = true;
                }
                return None;
            }

            if proc.pid as u32 != my_pid {
                let Ok(info) = get_info(proc) else {return None;};
                pb.inc(1);
                Some(info)
            } else {
                pb.inc(1);
                None
            }
        })
        .collect();
    pb.finish_and_clear();

    let vanished_processes_count = processes_count - processes_info.len();

    info!("");
    info!(
        "Scanned {} processes in {:?}",
        processes_info.len(),
        chrono.elapsed()
    );
    info!("{} vanished processes", vanished_processes_count);
    info!("");

    if cli.global_stats {
        let total_pfns = processes_info
            .iter()
            .map(|info| info.pfns.len())
            .sum::<usize>();
        info!(
            "Virtual pages: {total_pfns} ({} MiB)",
            total_pfns * page_size as usize / 1024 / 1024
        );
        info!("");
    }

    let mut processes_info = processes_info;
    while let Some(filter) = cli.custom_split.pop() {
        let mut splitter = ProcessSplitterCustomFilter::new(&filter).unwrap();
        splitter.split(&tree, processes_info);
        splitter.display();
        processes_info = splitter.collect_processes();
    }

    let processes_info: Vec<ProcessInfo> = if cli.split_uid {
        let mut splitter = ProcessSplitterUid::new();
        splitter.split(&tree, processes_info);
        splitter.display();
        splitter.collect_processes()
    } else {
        processes_info
    };

    let processes_info: Vec<ProcessInfo> = if let Some(var) = cli.split_env {
        let mut splitter = ProcessSplitterEnvVariable::new(var);
        splitter.split(&tree, processes_info);
        splitter.display();
        splitter.collect_processes()
    } else {
        processes_info
    };

    if !cli.split_pids.is_empty() {
        let mut splitter = ProcessSplitterPids::new(&cli.split_pids);
        splitter.split(&tree, processes_info);
        splitter.display();
    }

    if *hit_memory_limit.lock().unwrap() {
        warn!(
            "Hit memory limit ({} MiB), try increasing limit or filtering processes",
            mem_limit
        )
    }

    let vmhwm = my_process.status().unwrap().vmhwm.unwrap();
    let rssanon = my_process.status().unwrap().rssanon.unwrap();
    let vmrss = my_process.status().unwrap().vmrss.unwrap();
    let global_elapsed = global_chrono.elapsed();

    info!("vmhwm = {rssanon}");
    info!("vmrss = {vmrss}");
    info!("global_elapsed = {global_elapsed:?}");
}

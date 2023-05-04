#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unreachable_code)]
#![allow(unused_imports)]
#![allow(non_snake_case)]
#![feature(drain_filter)]

// Process groups memory statistics tool
// - Must run as root
// - Don't forget to set a memory limit (-m/--memory-limit) if you read shm pages (-r/--read-shm)
//
//
// TODO:
// - parallelize single pass
// - merge splitters into CustomSplitters
// - clap commands for splits
// - remove unwraps
// - custom hashset for u64?
//

use anyhow::Context;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::error::Error;
use indicatif::{ProgressBar, ProgressStyle};
use log::warn;
use log::{debug, error, info, Level};
use procfs::{
    page_size,
    process::{MMapPath, PageInfo, Pfn, Process},
    PhysicalPageFlags, Shm,
};
use rayon::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    hash::BuildHasherDefault,
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
type TheHash = std::collections::hash_map::DefaultHasher;

#[cfg(feature = "fnv")]
type TheHash = fnv::FnvHasher;

#[cfg(feature = "ahash")]
type TheHash = ahash::AHasher;

#[cfg(feature = "metrohash")]
type TheHash = metrohash::MetroHash;

#[cfg(feature = "fxhash")]
type TheHash = rustc_hash::FxHasher;

type ShmsMetadata =
    HashMap<procfs::Shm, Option<(HashSet<Pfn>, HashSet<(u64, u64)>, usize, usize)>, BuildHasherDefault<TheHash>>;

pub struct ProcessInfo {
    process: Process,
    uid: u32,
    environ: HashMap<OsString, OsString>,
    pfns: HashSet<Pfn, BuildHasherDefault<TheHash>>,
    swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>>,
    referenced_shms: HashSet<Shm>,
    rss: u64,
    vsz: u64,
    pte: u64,
    fds: usize,
}

pub struct ProcessGroupInfo {
    name: String,
    processes_info: Vec<ProcessInfo>,
    pfns: HashSet<Pfn, BuildHasherDefault<TheHash>>,
    swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>>,
    referenced_shm: HashSet<Shm>,
    pte: u64,
    fds: usize,
}

impl PartialEq for ProcessGroupInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct SmonInfo {
    pid: i32,
    sid: OsString,
    sga_size: u64,
    large_pages: String,
    processes: u64,
    pga_size: u64,
    //sga_shm: Shm,
    //sga_pfns: HashSet<Pfn>,
}

// return info memory maps info for standard process or None for kernel process
fn get_process_info(
    process: Process,
    shms_metadata: &ShmsMetadata,
) -> Result<ProcessInfo, Box<dyn std::error::Error>> {
    if process.cmdline()?.is_empty() {
        // already handled in main
        return Err(String::from("No info for kernel process"))?;
    }

    let page_size = procfs::page_size();

    // physical memory pages
    let mut pfns: HashSet<Pfn, BuildHasherDefault<TheHash>> = Default::default();
    // swap type, offset
    let mut swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>> = HashSet::default();

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

    let memory_maps = snap::get_memory_maps_for_process(&process, true)?;

    let mut referenced_shms = HashSet::new();

    for (memory_map, pages) in memory_maps.iter() {
        let size = memory_map.address.1 - memory_map.address.0;
        vsz += size;
        let max_pages = size / page_size;

        if let MMapPath::Vsys(key) = &memory_map.pathname {
            // shm
            let mut found = false;

            for shm in shms_metadata.keys() {
                if shm.key == *key && shm.shmid == memory_map.inode {
                    referenced_shms.insert(*shm);
                    found = true;
                    break;
                }
            }
            if !found {
                warn!(
                    "Cant' find shm key {:?} shmid {:?} for pid {}",
                    key, memory_map.inode, process.pid
                );
            }
        } else {
            // not shm
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
        }
    } // end for memory_maps

    let uid = process.uid()?;
    let env = process.environ()?;

    Ok(ProcessInfo {
        process,
        uid,
        environ: env,
        pfns,
        referenced_shms,
        swap_pages,
        rss,
        vsz,
        pte,
        fds,
    })
}

fn processes_group_info(
    processes_info: Vec<ProcessInfo>,
    name: String,
    shms_metadata: &ShmsMetadata,
) -> ProcessGroupInfo {
    let mut pfns: HashSet<Pfn, BuildHasherDefault<TheHash>> = HashSet::default();
    let mut swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>> = HashSet::default();
    let mut referenced_shm = HashSet::new();
    let mut pte = 0;
    let mut fds = 0;

    for process_info in &processes_info {
        pfns.par_extend(&process_info.pfns);
        swap_pages.par_extend(&process_info.swap_pages);
        referenced_shm.extend(&process_info.referenced_shms);
        pte += process_info.pte;
        fds += process_info.fds;
    }

    ProcessGroupInfo {
        name,
        processes_info,
        pfns,
        swap_pages,
        referenced_shm,
        pte,
        fds,
    }
}

mod splitters {
    use std::{
        collections::{BTreeMap, HashMap, HashSet},
        ffi::{OsStr, OsString},
        hash::BuildHasherDefault,
    };

    use anyhow::{bail, Context};
    use indicatif::ProgressBar;
    use itertools::Itertools;
    use log::{info, warn, debug};
    use procfs::{page_size, process::Pfn, Shm};
    use rayon::prelude::*;
    use rustc_hash::FxHasher;

    use crate::{
        filters::{self, Filter},
        process_tree::ProcessTree,
        processes_group_info, ProcessGroupInfo, ProcessInfo, ShmsMetadata, TheHash,
    };

    pub trait ProcessSplitter<'a> {
        fn name(&self) -> String;
        type GroupIter<'b: 'a>: Iterator<Item = &'a ProcessGroupInfo>
        where
            Self: 'b;
        fn __split(
            &mut self,
            tree: &ProcessTree,
            shm_metadata: &ShmsMetadata,
            processes: Vec<ProcessInfo>,
        );
        fn iter_groups(&self) -> Self::GroupIter<'_>;
        fn collect_processes(self) -> Vec<ProcessInfo>;

        fn split(
            &mut self,
            tree: &ProcessTree,
            shms_metadata: &ShmsMetadata,
            processes: Vec<ProcessInfo>,
        ) {
            let chrono = std::time::Instant::now();
            self.__split(tree, shms_metadata, processes);
            debug!("Split by {}: took {:?}", self.name(), chrono.elapsed());
        }

        fn display(&'a self, shm_metadata: &ShmsMetadata) {
            let chrono = std::time::Instant::now();

            let mut info = Vec::new();
            let pb = ProgressBar::new(self.iter_groups().count() as u64);
            for group_1 in self.iter_groups() {
                let mut other_pfns: HashSet<Pfn, BuildHasherDefault<TheHash>> = HashSet::default();
                let mut other_swap: HashSet<(u64, u64), BuildHasherDefault<TheHash>> =
                    HashSet::default();
                let mut other_referenced_shm: HashSet<Shm> = HashSet::new();
                for group_other in self.iter_groups() {
                    if group_1 != group_other {
                        other_pfns.par_extend(&group_other.pfns);
                        other_swap.par_extend(&group_other.swap_pages);
                        other_referenced_shm.par_extend(&group_other.referenced_shm);
                    }
                }
                for (shm, meta) in shm_metadata {
                    match meta {
                        Some((shm_pfns, swap_pages, _pages_4k, _pages_2M)) => {
                            if other_referenced_shm.contains(shm) {
                                other_pfns.par_extend(shm_pfns);
                            }
                        },
                        None => (),
                    }
                }

                let mut group_1_pfns = group_1.pfns.clone();
                for (shm, meta) in shm_metadata {
                    match meta {
                        Some((shm_pfns, swap_pages, _pages_4k, _pages_2M)) => {
                            if group_1.referenced_shm.contains(shm) {
                                group_1_pfns.par_extend(shm_pfns);
                            }
                        },
                        None => (),
                    }
                }
                let processes_count = group_1.processes_info.len();
                let mem_rss = group_1_pfns.len() as u64 * procfs::page_size() / 1024 / 1024;
                let mem_uss = group_1_pfns.difference(&other_pfns).count() as u64
                    * procfs::page_size()
                    / 1024
                    / 1024;

                let swap_rss = group_1.swap_pages.len() as u64 * procfs::page_size() / 1024 / 1024;
                let swap_uss = group_1.swap_pages.difference(&other_swap).count() as u64
                    * procfs::page_size()
                    / 1024
                    / 1024;

                // TODO: no differences for shm?
                let shm_mem: u64 = group_1
                    .referenced_shm
                    .iter()
                    .map(|shm| shm.rss)
                    .sum::<u64>()
                    / 1024
                    / 1024;
                let shm_swap: u64 = group_1
                    .referenced_shm
                    .iter()
                    .map(|shm| shm.swap)
                    .sum::<u64>()
                    / 1024
                    / 1024;

                info.push((
                    group_1.name.clone(),
                    processes_count,
                    mem_rss,
                    mem_uss,
                    swap_rss,
                    swap_uss,
                    shm_mem,
                    shm_swap,
                ));
                pb.inc(1);
            }
            pb.finish_and_clear();

            // sort by mem RSS
            info.sort_by(|a, b| b.2.cmp(&a.2));

            println!("Process groups by {} (MiB)", self.name());
            println!("group_name                     #procs         RSS         USS   SWAP RSS   SWAP USS    SHM MEM   SHM SWAP",);
            println!("=========================================================================================================");
            for (name, processes_count, mem_rss, mem_uss, swap_rss, swap_uss, shm_mem, shm_swap) in
                info
            {
                println!(
                    "{:<30}  {:>5}  {:>10}  {:>10} {:>10} {:>10} {:>10} {:>10}",
                    name, processes_count, mem_rss, mem_uss, swap_rss, swap_uss, shm_mem, shm_swap
                );
            }
            debug!("Display split by {}: {:?}", self.name(), chrono.elapsed());
            println!("");
        }
    }

    pub struct ProcessSplitterCustomFilter {
        name: String,
        filters: Vec<Box<dyn Filter>>,
        names: Vec<String>,
        groups: HashMap<String, ProcessGroupInfo>,
    }
    impl ProcessSplitterCustomFilter {
        pub fn new(input: &str) -> anyhow::Result<Self> {
            if !input.is_ascii() {
                bail!("Filter must be ASCII");
            }

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

        fn __split(
            &mut self,
            tree: &ProcessTree,
            shms_metadata: &ShmsMetadata,
            mut processes: Vec<ProcessInfo>,
        ) {
            for (group_name, filter) in self.names.iter().zip(&self.filters) {
                let some_processes = processes
                    .drain_filter(|p| filter.eval(&p.process, tree))
                    .collect();
                let process_group_info =
                    processes_group_info(some_processes, group_name.clone(), shms_metadata);
                self.groups.insert(group_name.clone(), process_group_info);
            }

            // remaining processes not captured by any filter
            let other_info = processes_group_info(processes, "Other".to_string(), shms_metadata);
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
        fn __split(
            &mut self,
            _tree: &ProcessTree,
            shms_metadata: &ShmsMetadata,
            mut processes: Vec<ProcessInfo>,
        ) {
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
                let process_group_info = processes_group_info(some_processes, name, shms_metadata);
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
        fn __split(
            &mut self,
            _tree: &ProcessTree,
            shms_metadata: &ShmsMetadata,
            processes: Vec<ProcessInfo>,
        ) {
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
            let process_group_info_0 =
                processes_group_info(processes_info_0, name_0, shms_metadata);
            let process_group_info_1 =
                processes_group_info(processes_info_1, name_1, shms_metadata);

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
        fn __split(
            &mut self,
            _tree: &ProcessTree,
            shms_metadata: &ShmsMetadata,
            mut processes: Vec<ProcessInfo>,
        ) {
            let uids: HashSet<u32> = processes.iter().map(|p| p.uid).collect();

            for uid in uids {
                let username = users::get_user_by_uid(uid);
                let username = match username {
                    Some(username) => username.name().to_string_lossy().to_string(),
                    None => format!("{uid}"),
                };
                let processes_info: Vec<ProcessInfo> =
                    processes.drain_filter(|p| p.uid == uid).collect();
                let group_info = processes_group_info(processes_info, username, shms_metadata);
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
    struct CommFilter {
        pub comm: String,
    }
    impl Filter for CommFilter {
        fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
            match p.stat() {
                Ok(stat) => stat.comm == self.comm,
                Err(_) => false,
            }
        }
    }

    #[derive(Debug)]
    struct UidFilter {
        pub uid: u32,
    }
    impl Filter for UidFilter {
        fn eval(&self, p: &Process, _: &ProcessTree) -> bool {
            match p.uid() {
                Ok(uid) => uid == self.uid,
                Err(_) => false,
            }
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

        let closing = find_match_par(input, opening)?;
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
            "comm" => Ok((Box::new(CommFilter { comm: inner }), ate)),
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
    let myself = std::env::current_exe()?;

    let mut lib = home.to_os_string();
    lib.push("/lib");

    let output = Command::new(myself)
        .env("LD_LIBRARY_PATH", lib)
        .env("ORACLE_SID", sid)
        .env("ORACLE_HOME", home)
        .uid(uid)
        .arg("get-db-info")
        .args(["--pid", &format!("{pid}")])
        .output()?;
        
    if !output.status.success() {
        return Err(format!("Can't get info for {sid:?} {uid} {home:?}: {:?}", output))?;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    let smon_info: SmonInfo = serde_json::from_str(&stdout)?;
    Ok(smon_info)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
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
    - comm(<comm>)
    - env_k(<env key>)
    - env_kv(<env key,env value>)
Limitation:
    - ALL filters require trailing parenthesis, even true/false
    - Spaces are not allowed before/after commas
    - Characters can't be escaped at the moment
Examples:
    - All `cat` processes: comm(cat)
    - All processes for user 1000: uid(1000)
    - All processes that have a `DISPLAY` env variable (whatever its value is): env_k(DISPLAY)
    - All processes that have a `SHELL` env variable with value `/bin/bash`: env_kv(SHELL,/bin/bash)
    - All non-root processes that have a `DISPLAY` env variable: and(not(uid(0)),env_k(DISPLAY))
    ";

    #[derive(Parser, Debug)]
    #[command(author, version, about, long_about = None, after_help = AFTER_HELP)]
    struct Cli {
        #[arg(long, hide(true))]
        scan_kpageflags: bool,

        #[arg(short, long)]
        mem_limit: Option<u64>,

        #[arg(short, long)]
        threads: Option<usize>,

        #[arg(short, long)]
        global_stats: bool,

        #[arg(
            short,
            long,
            help = "Filter to scan only a subset of processes. See below for syntax"
        )]
        filter: Option<String>,

        #[arg(
            short,
            long,
            help = "List processes that will be scanned, useful to validate filters"
        )]
        list_processes: bool,

        #[arg(short, long, action = clap::ArgAction::Set, default_value_t = false, help = "Force read PFN for shm, even if shm is in swap")]
        force_read_shm: bool,

        #[command(subcommand)]
        commands: Commands,
    }

    #[derive(Debug, Subcommand)]
    enum Commands {
        #[command(hide = true)]
        GetDbInfo {
            #[arg(long, required = true)]
            pid: i32,
        },
        /// Single threaded process scan, can't do multiple groups, but memory efficient
        Single,
        /// Multi threaded process scan, multiple groups, memory hungry
        Groups {
            #[arg(short = 'e', long)]
            split_env: Option<String>,

            #[arg(short = 'u', long)]
            split_uid: bool,

            #[arg(short = 'p', long, action = clap::ArgAction::Append)]
            split_pids: Vec<i32>,

            #[arg(long, help = "Comma separated list of filters, evaluated in order")]
            split_custom: Vec<String>,
        },
    }

    let cli = Cli::parse();

    if let Commands::GetDbInfo { pid } = cli.commands {
        // oracle shouldn't run as root
        assert_ne!(users::get_effective_uid(), 0);

        // subprogram to connect to instance and print sga size
        // We must have the correct context (user, env vars) to connect to database
        let (sga_size, processes, pga_size, large_pages) = snap::get_db_info().unwrap();

        let sid = std::env::var_os("ORACLE_SID").expect("Missing ORACLE_SID");

        let smon_info: SmonInfo = SmonInfo { pid, sid: sid.clone(), sga_size, large_pages, processes, pga_size };
        let out = serde_json::to_string(&smon_info).expect(&format!("Can't serialize SmonInfo for {sid:?}"));
        println!("{out}");

        // print value, can't use logger here
        // parent will grab that value in `get_smon_info`
        //println!("{sga_size} {processes} {pga_size} {large_pages}");
        std::process::exit(0);
    }
    // can't print anything before that line

    //dbg!(&cli);

    let mem_limit = if let Some(m) = cli.mem_limit {
        m
    } else {
        let meminfo = procfs::Meminfo::new().unwrap();
        meminfo.mem_available.unwrap() / 1024 / 1024 / 2
    };
    debug!("Memory limit: {mem_limit} MiB");
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

    debug!("Using {threads} threads");
    debug!("");

    // Main program starts here
    if users::get_effective_uid() != 0 {
        error!("Run as root");
        std::process::exit(1);
    }

    let page_size = procfs::page_size();

    println!("Scanning /proc/kpageflags...");
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

    // find smons processes, and for each spawn a new process in the correct context to get database info
    println!("Scanning Oracle instances...");
    let mut instances: Vec<SmonInfo> = snap::find_smons()
        .iter()
        .filter_map(|(pid, uid, sid, home)| {
            let smon_info = get_smon_info(*pid, *uid, sid.as_os_str(), home.as_os_str());

            match smon_info {
            Ok(x) => Some(x),
                Err(e) => {
                    warn!("Can't get DB info for {sid:?}: {e:?}");
                    None
                },
            }
        })
        .collect();

    instances.sort_by(|a, b| a.sga_size.cmp(&b.sga_size).reverse());

    if !instances.is_empty() {
        println!("Oracle instances (MiB):");
        println!("SID                  SGA         PGA  PROCESSES  LARGE_PAGES");
        println!("============================================================");
        for instance in &instances {
            println!(
                "{:<12} {:>12} {:>10} {:>10} {:>12}",
                instance.sid.to_string_lossy(),
                instance.sga_size / 1024 / 1024,
                instance.pga_size / 1024 /1024,
                instance.processes,
                instance.large_pages,
            );
        }
        println!("");
    } else {
        println!("Can't locate any Oracle instance");
        println!("");
    }

    println!("Scanning shm...");
    for shm in procfs::Shm::new().expect("Can't read /dev/sysvipc/shm") {
        // dummy scan shm so rss is in sync with number of pages
        let x = snap::shm2pfns(&all_physical_pages, &shm, cli.force_read_shm).unwrap();
    }

    let mut shms_metadata: ShmsMetadata = HashMap::default();
    for shm in procfs::Shm::new().expect("Can't read /dev/sysvipc/shm") {
        // TODO remove unwrap for Result
        let x = snap::shm2pfns(&all_physical_pages, &shm, cli.force_read_shm).unwrap();

        shms_metadata.insert(shm, x);
    }

    if !shms_metadata.is_empty() {
        let mut shms: Vec<Shm> = shms_metadata.keys().copied().collect();
        shms.sort_by(|a, b| a.size.cmp(&b.size).reverse());

        println!("Shared memory segments (MiB):");
        println!("         key           id       Size        RSS       4k/2M          SWAP   USED%        SID",);
        println!("============================================================================================",);
        for shm in &shms {
            let mut sid_list = Vec::new();
            for instance in &instances {
                let Ok(process) = Process::new(instance.pid) 
                 else {
                    continue;
                };
                let Ok(process_info) = get_process_info(process, &shms_metadata) else {
                    continue;
                };

                if process_info.referenced_shms.contains(shm) {
                    sid_list.push(instance.sid.to_string_lossy().to_string());
                }
            }

            // TODO: remove unwrap
            let (pages_4k, pages_2M) = match shms_metadata.get(shm).unwrap() {
                Some((_pfns, _swap_pages, pages_4k, pages_2M)) => (format!("{}", pages_4k), format!("{}", pages_2M)),
                None => ("-".to_string(), "-".to_string()),
            };

            println!(
                "{:>12} {:>12} {:>10} {:>10} {:>8}/{:<8} {:>7} {:>7.2} {:>10}",
                shm.key,
                shm.shmid,
                shm.size / 1024 / 1024,
                shm.rss / 1024 / 1024,
                pages_4k,
                pages_2M,
                shm.swap / 1024 / 1024,
                (shm.rss + shm.swap) as f32 / shm.size as f32 * 100.,
                sid_list.join(" ")
            );
            // USED% can be >100% if size is not aligned with the underling pages: in that case, size<rss+swap
        }
        println!("");
    } else {
        println!("Can't locate any shared memory segment");
        println!("");
    }

    // probably incorrect?
    // size of kernel structures
    //let current_kernel = procfs::sys::kernel::Version::current().unwrap();
    //let (fd_size, task_size) =
    //    snap::get_kernel_datastructure_size(current_kernel).expect("Unknown kernel");

    //let mut kpagecount = procfs::KPageCount::new().expect("Can't open /proc/kpagecount");

    // processes are scanned once and reused to get a more consistent view
    let mut kernel_processes_count = 0;
    let all_processes: Vec<Process> = procfs::process::all_processes()
        .unwrap()
        .filter_map(|p| 
        match p {
            Ok(p) => Some(p),
            Err(e) => {
                match e {
                    procfs::ProcError::NotFound(_) => None,
                    x => {
                        log::error!("Can't read process {x:?}");
                        std::process::exit(1);
                    },
                }
            }
        }
        )
        .collect();
    let all_processes_count = all_processes.len();
    info!("Total processes {all_processes_count}");
    let tree = ProcessTree::new(&all_processes);

    // exclude kernel procs
    let processes: Vec<Process> = all_processes
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
    info!("Excluded {} kernel processes", kernel_processes_count);

    let processes: Vec<Process> = if let Some(filter) = cli.filter {
        let (f, ate) = filters::parse(&filter).unwrap();
        if filter.chars().count() != ate {
            warn!("Ate {ate}, but filter is {} chars", filter.chars().count());
        }

        let processes: Vec<Process> = processes.into_iter().filter(|p| f.eval(p, &tree)).collect();
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
        processes
    };
    //println!("");

    if cli.list_processes {
        println!("       uid        pid comm");
        println!("==========================");
        for (uid, pid, comm) in processes
            .iter()
            .filter_map(|p| Some((p.uid().ok()?, p.pid, p.stat().ok()?.comm)))
        {
            println!("{uid:>10} {pid:>10} {comm}");
        }
        println!("");
    }

    let my_pid = std::process::id();
    let my_process = procfs::process::Process::new(my_pid as i32).unwrap();

    match cli.commands {
        Commands::GetDbInfo  { .. } => unreachable!(),
        Commands::Single => {
            scan_single(
                my_process,
                global_chrono,
                mem_limit,
                processes,
                &tree,
                &shms_metadata,
            );
        }
        Commands::Groups {
            split_env,
            split_uid,
            split_pids,
            split_custom,
        } => {
            scan_groups(
                my_process,
                global_chrono,
                mem_limit,
                processes,
                &tree,
                &shms_metadata,
                split_env,
                split_uid,
                split_pids,
                split_custom,
            );
        }
    }

    fn scan_single(
        my_process: Process,
        global_chrono: std::time::Instant,
        mem_limit: u64,
        processes: Vec<Process>,
        tree: &ProcessTree,
        shms_metadata: &ShmsMetadata,
    ) {
        let processes_count = processes.len();
        let single_chrono = std::time::Instant::now();
        let hit_memory_limit = Arc::new(Mutex::new(false));

        let mut mem_pages: HashSet<Pfn, BuildHasherDefault<TheHash>> = HashSet::default();
        let mut swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>> = HashSet::default();
        let mut referenced_shm: HashSet<Shm> = HashSet::new();
        let mut scanned_processes = 0;
        let mut vanished = 0;
        let pb = ProgressBar::new(processes_count as u64);
        pb.set_style(ProgressStyle::with_template("{msg} {wide_bar} {pos}/{len}").unwrap());
        for process in processes {
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
                break;
            }
            let process_info = match get_process_info(process, shms_metadata) {
                Ok(info) => info,
                Err(_) => {
                    vanished += 1;
                    continue;
                }
            };
            scanned_processes += 1;

            mem_pages.par_extend(&process_info.pfns);
            swap_pages.par_extend(&process_info.swap_pages);
            referenced_shm.extend(process_info.referenced_shms);
            pb.inc(1);
        }
        pb.finish_and_clear();

        let rss = mem_pages.len() as u64 * procfs::page_size() / 1024 / 1024;
        let swap = swap_pages.len() as u64 * procfs::page_size() / 1024 / 1024;
        let shm_mem: u64 = referenced_shm.iter().map(|shm| shm.rss).sum::<u64>() / 1024 / 1024;
        let shm_swap: u64 = referenced_shm.iter().map(|shm| shm.swap).sum::<u64>() / 1024 / 1024;

        println!(
            "{} processes scanned in {:?}",
            scanned_processes,
            single_chrono.elapsed()
        );
        info!("");
        info!("Statistics:");
        info!("mem RSS: {rss}");
        info!("swap RSS: {swap}");
        info!("shm mem: {shm_mem}");
        info!("shm swap: {shm_swap}");

        finalize(hit_memory_limit, mem_limit, &my_process, global_chrono);
    }

    fn scan_groups(
        my_process: Process,
        global_chrono: std::time::Instant,
        mem_limit: u64,
        processes: Vec<Process>,
        tree: &ProcessTree,
        shms_metadata: &ShmsMetadata,
        split_env: Option<String>,
        split_uid: bool,
        split_pids: Vec<i32>,
        mut split_custom: Vec<String>,
    ) {
        let processes_count = processes.len();
        let hit_memory_limit = Arc::new(Mutex::new(false));
        let chrono = std::time::Instant::now();
        println!("\nScanning {processes_count} processes");
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

                if proc.pid != my_process.pid {
                    let Ok(info) = get_process_info(proc, shms_metadata) else {return None;};
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

        println!(
            "Scanned {} processes in {:?}",
            processes_info.len(),
            chrono.elapsed()
        );
        info!("{} processe(s) vanished", vanished_processes_count);
        info!("");

        let mut processes_info = processes_info;
        while let Some(filter) = split_custom.pop() {
            let mut splitter = ProcessSplitterCustomFilter::new(&filter).unwrap();
            splitter.split(tree, shms_metadata, processes_info);
            splitter.display(shms_metadata);
            processes_info = splitter.collect_processes();
        }

        let processes_info: Vec<ProcessInfo> = if split_uid {
            let mut splitter = ProcessSplitterUid::new();
            splitter.split(tree, shms_metadata, processes_info);
            splitter.display(shms_metadata);
            splitter.collect_processes()
        } else {
            processes_info
        };

        let processes_info: Vec<ProcessInfo> = if let Some(var) = split_env {
            let mut splitter = ProcessSplitterEnvVariable::new(var);
            splitter.split(tree, shms_metadata, processes_info);
            splitter.display(shms_metadata);
            splitter.collect_processes()
        } else {
            processes_info
        };

        if !split_pids.is_empty() {
            let mut splitter = ProcessSplitterPids::new(&split_pids);
            splitter.split(tree, shms_metadata, processes_info);
            splitter.display(shms_metadata);
        }

        finalize(hit_memory_limit, mem_limit, &my_process, global_chrono);
    }

    fn finalize(
        hit_memory_limit: Arc<Mutex<bool>>,
        mem_limit: u64,
        my_process: &Process,
        global_chrono: std::time::Instant,
    ) {
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

        info!("");
        info!("vmhwm = {rssanon}");
        info!("vmrss = {vmrss}");
        info!("global_elapsed = {global_elapsed:?}");
    }
}

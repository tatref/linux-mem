use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::{OsStr, OsString},
    hash::BuildHasherDefault,
};

use anyhow::{bail, Context};
use indicatif::ProgressBar;
use log::{debug, warn};
use procfs::{process::Pfn, Shm};
use rayon::prelude::*;

use crate::{
    filters::{self, Filter},
    get_processes_group_info, FxHasher, ProcessGroupInfo, ProcessInfo,
};
use crate::{process_tree::ProcessTree, ShmsMetadata};

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

        use crate::tmpfs::format_units_MiB;
        use tabled::Tabled;

        #[derive(Tabled)]
        struct ProcessGroupDisplayRow {
            group_name: String,
            procs: usize,
            #[tabled(display_with = "format_units_MiB")]
            mem_rss: u64,
            #[tabled(display_with = "format_units_MiB")]
            mem_anon: u64,
            #[tabled(display_with = "format_units_MiB")]
            mem_uss: u64,
            #[tabled(display_with = "format_units_MiB")]
            swap_anon: u64,
            #[tabled(display_with = "format_units_MiB")]
            swap_rss: u64,
            #[tabled(display_with = "format_units_MiB")]
            swap_uss: u64,
            #[tabled(display_with = "format_units_MiB")]
            shm_mem: u64,
            #[tabled(display_with = "format_units_MiB")]
            shm_swap: u64,
        }

        let mut display_info: Vec<ProcessGroupDisplayRow> = Vec::new();

        let pb = ProgressBar::new(self.iter_groups().count() as u64);
        for group_1 in self.iter_groups() {
            let mut other_pfns: HashSet<Pfn, BuildHasherDefault<FxHasher>> = HashSet::default();
            let mut other_swap: HashSet<(u64, u64), BuildHasherDefault<FxHasher>> =
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
                    Some((_shm_pfns, _swap_pages, _pages_4k, _pages_2M)) => {
                        if other_referenced_shm.contains(shm) {
                            //other_pfns.par_extend(shm_pfns);
                        }
                    }
                    None => (),
                }
            }

            let group_1_pfns = group_1.pfns.clone();
            for (shm, meta) in shm_metadata {
                match meta {
                    Some((_shm_pfns, _swap_pages, _pages_4k, _pages_2M)) => {
                        if group_1.referenced_shm.contains(shm) {
                            // TODO: we count shm as rss
                            // do something else?
                            //group_1_pfns.par_extend(shm_pfns);
                        }
                    }
                    None => (),
                }
            }
            let processes_count = group_1.processes_info.len();
            let mem_rss = group_1_pfns.len() as u64 * procfs::page_size();
            let mem_anon = group_1.anon_pfns.len() as u64 * procfs::page_size();
            let mem_uss = group_1_pfns.difference(&other_pfns).count() as u64 * procfs::page_size();

            let swap_rss = group_1.swap_pages.len() as u64 * procfs::page_size();
            let swap_anon = group_1.anon_swap_pages.len() as u64 * procfs::page_size();
            let swap_uss =
                group_1.swap_pages.difference(&other_swap).count() as u64 * procfs::page_size();

            // TODO: no differences for shm?
            let shm_mem: u64 = group_1
                .referenced_shm
                .iter()
                .map(|shm| shm.rss)
                .sum::<u64>();
            let shm_swap: u64 = group_1
                .referenced_shm
                .iter()
                .map(|shm| shm.swap)
                .sum::<u64>();

            display_info.push(ProcessGroupDisplayRow {
                group_name: group_1.name.clone(),
                procs: processes_count,
                mem_rss,
                mem_anon,
                mem_uss,
                swap_rss,
                swap_anon,
                swap_uss,
                shm_mem,
                shm_swap,
            });
            pb.inc(1);
        }
        pb.finish_and_clear();

        // sort by mem RSS
        display_info.sort_by(|a, b| b.mem_rss.cmp(&a.mem_rss));

        let mut table = tabled::Table::new(&display_info);
        table.with(tabled::settings::Style::sharp());

        println!("{}", self.name());
        println!("{table}");

        debug!("Display split by {}: {:?}", self.name(), chrono.elapsed());
        println!();
    }
}

pub struct ProcessSplitterCustomFilter {
    pub name: String,
    pub filters: Vec<Box<dyn Filter>>,
    pub names: Vec<String>,
    pub groups: HashMap<String, ProcessGroupInfo>,
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
                .extract_if(.., |p| filter.eval(&p.process, tree))
                .collect();
            let process_group_info =
                get_processes_group_info(some_processes, group_name, shms_metadata);
            self.groups.insert(group_name.clone(), process_group_info);
        }

        // remaining processes not captured by any filter
        let other_info = get_processes_group_info(processes, "Other", shms_metadata);
        self.groups.insert("Other".to_string(), other_info);
    }

    fn iter_groups<'x>(&'a self) -> Self::GroupIter<'a> {
        self.groups.values()
    }

    fn collect_processes(mut self) -> Vec<ProcessInfo> {
        self.groups
            .par_drain()
            .flat_map(|(_k, process_group_info)| process_group_info.processes_info)
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
                .extract_if(.., |p| p.environ.get(&self.var) == sid.as_ref())
                .collect();
            let name = format!(
                "{:?}",
                sid.as_ref().map(|os| os.to_string_lossy().to_string())
            );
            let process_group_info = get_processes_group_info(some_processes, &name, shms_metadata);
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
            .flat_map(|(_k, process_group_info)| process_group_info.processes_info)
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
            let username = uzers::get_user_by_uid(uid);
            let username = match username {
                Some(username) => username.name().to_string_lossy().to_string(),
                None => format!("{uid}"),
            };
            let processes_info: Vec<ProcessInfo> =
                processes.extract_if(.., |p| p.uid == uid).collect();
            let group_info = get_processes_group_info(processes_info, &username, shms_metadata);
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

#![allow(dead_code)]
#![allow(unused_variables)]
#![feature(drain_filter)]

// TODO:
// - bench memory usage
// - hash algos
//   - std
//   - https://crates.io/crates/fnv
//   - ahash
//   - https://crates.io/crates/xxhash-rust
//   - https://crates.io/crates/metrohash
// - filters
// - process stats after scan (vanished, kernel...)
// - proper logging
// - remove unwraps

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

use crate::splitters::{
    ProcessSplitter, ProcessSplitterByEnvVariable, ProcessSplitterByPids, ProcessSplitterByUid,
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

mod splitters {
    use std::{
        collections::{BTreeMap, HashMap, HashSet},
        ffi::{OsStr, OsString},
    };

    use itertools::Itertools;
    use log::info;
    use rayon::prelude::*;

    use crate::{processes_group_info, ProcessGroupInfo, ProcessGroupPfns, ProcessInfo};

    pub trait ProcessSplitter<'a> {
        fn name(&self) -> String;
        type GroupIter<'b: 'a>: Iterator<Item = &'a ProcessGroupInfo>
        where
            Self: 'b;
        fn __split(&mut self, processes: Vec<ProcessInfo>);
        fn iter_groups<'b>(&'b self) -> Self::GroupIter<'b>;
        fn collect_processes(self) -> Vec<ProcessInfo>;

        fn split(&mut self, processes: Vec<ProcessInfo>) {
            let chrono = std::time::Instant::now();
            self.__split(processes);
            info!("Split by {}: {:?}", self.name(), chrono.elapsed());
        }

        fn display(&'a self) {
            let chrono = std::time::Instant::now();
            info!("Process groups by {}", self.name());
            info!("group_name                     #procs     RSS MiB     USS MiB",);
            info!("=============================================================");
            for group_1 in self.iter_groups() {
                let mut other_pfns: ProcessGroupPfns = HashSet::default();
                for group_2 in self.iter_groups() {
                    if group_1 != group_2 {
                        other_pfns.par_extend(&group_2.pfns);
                    }
                }

                let count = group_1.processes_info.len();
                let rss = group_1.pfns.len() as u64 * procfs::page_size() / 1024 / 1024;
                let uss = group_1.pfns.difference(&other_pfns).count() as u64 * procfs::page_size()
                    / 1024
                    / 1024;

                info!(
                    "{:<30}  {:>5}  {:>10}  {:>10}",
                    group_1.name, count, rss, uss
                );
            }
            info!("Display split by {}: {:?}", self.name(), chrono.elapsed());
            info!("");
        }
    }

    pub struct ProcessSplitterByEnvVariable {
        var: OsString,
        groups: HashMap<Option<OsString>, ProcessGroupInfo>,
    }
    impl ProcessSplitterByEnvVariable {
        pub fn new<S: AsRef<OsStr>>(var: S) -> Self {
            Self {
                groups: HashMap::new(),
                var: var.as_ref().to_os_string(),
            }
        }
    }

    impl<'a> ProcessSplitter<'a> for ProcessSplitterByEnvVariable {
        type GroupIter<'b: 'a> =
            std::collections::hash_map::Values<'a, Option<OsString>, ProcessGroupInfo>;

        fn name(&self) -> String {
            format!("environment variable {}", self.var.to_string_lossy())
        }
        fn __split(&mut self, mut processes: Vec<ProcessInfo>) {
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
        fn collect_processes(self) -> Vec<ProcessInfo> {
            self.groups
                .into_values()
                .flat_map(|group| group.processes_info)
                .collect()
        }
    }
    pub struct ProcessSplitterByPids {
        pids: Vec<i32>,
        groups: BTreeMap<u8, ProcessGroupInfo>,
    }

    impl ProcessSplitterByPids {
        pub fn new(pids: &[i32]) -> Self {
            Self {
                pids: pids.to_vec(),
                groups: BTreeMap::new(),
            }
        }
    }
    impl<'a> ProcessSplitter<'a> for ProcessSplitterByPids {
        type GroupIter<'b: 'a> = std::collections::btree_map::Values<'a, u8, ProcessGroupInfo>;

        fn name(&self) -> String {
            format!("PID list")
        }
        fn __split(&mut self, processes: Vec<ProcessInfo>) {
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
    pub struct ProcessSplitterByUid {
        groups: BTreeMap<u32, ProcessGroupInfo>,
    }

    impl ProcessSplitterByUid {
        pub fn new() -> Self {
            Self {
                groups: BTreeMap::new(),
            }
        }
    }
    impl<'a> ProcessSplitter<'a> for ProcessSplitterByUid {
        type GroupIter<'b: 'a> = std::collections::btree_map::Values<'a, u32, ProcessGroupInfo>;

        fn name(&self) -> String {
            format!("UID")
        }
        fn __split(&mut self, mut processes: Vec<ProcessInfo>) {
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

    #[derive(Parser, Debug)]
    #[command(author, version, about, long_about = None)]
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
    }

    let cli = Cli::parse();

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
    //println!("type={}", std::any::type_name::<ProcessInfoPfns>());

    let mem_limit = if let Some(m) = cli.mem_limit {
        m
    } else {
        let meminfo = procfs::Meminfo::new().unwrap();
        meminfo.mem_available.unwrap() / 1024 / 1024
    };
    info!("Memory limit: {mem_limit} MiB");
    let threads = if let Some(t) = cli.threads {
        t
    } else {
        std::thread::available_parallelism()
            .unwrap_or(NonZeroUsize::new(1).unwrap())
            .get()
    };
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()
        .unwrap();

    info!("Using {threads} threads");
    // Main program starts here
    if users::get_effective_uid() != 0 {
        error!("Run as root");
        panic!();
    }

    let page_size = procfs::page_size();

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
                let pfns: Vec<Pfn> = (start.0..end.0).map(|pfn| Pfn(pfn)).collect();

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
    let chrono = std::time::Instant::now();
    let all_processes: Vec<_> = procfs::process::all_processes().unwrap().collect();
    let processes_count = all_processes.len();

    info!("Scanning {processes_count} processes");
    let pb = ProgressBar::new(processes_count as u64);
    pb.set_style(ProgressStyle::with_template("{msg} {wide_bar} {pos}/{len}").unwrap());
    let processes_info: Vec<ProcessInfo> = all_processes
        .into_par_iter()
        //.progress_count(processes_count as u64)
        .filter_map(|proc| {
            let my_rss = my_process.status().unwrap().vmrss.unwrap() / 1024;
            pb.set_message(format!("{my_rss}/{mem_limit} MiB"));

            if my_rss > mem_limit {
                while let Ok(mut guard) = hit_memory_limit.try_lock() {
                    if !*guard {
                        warn!(
"Hit memory limit ({} MiB), try increasing limit or filtering processes",
                            mem_limit
                            );
                        *guard = true;
                    }
                    break;
                }
                return None;
            }

            let Ok(proc) = proc else { return None;};
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

    info!("");
    info!(
        "Scanned {} processes in {:?}",
        processes_info.len(),
        chrono.elapsed()
    );

    let total_pfns = processes_info
        .iter()
        .map(|info| info.pfns.len())
        .sum::<usize>();
    let max_pfns = processes_info
        .iter()
        .map(|info| info.pfns.len())
        .max()
        .unwrap();
    info!(
        "Total PFNs: {total_pfns} ({} MiB)",
        total_pfns / 1024 / 1024
    );
    info!(
        "Max PFNs: {max_pfns} ({} MiB)",
        max_pfns * page_size as usize / 1024 / 1024
    );
    info!("");

    let processes_info: Vec<ProcessInfo> = if cli.split_uid {
        let mut splitter = ProcessSplitterByUid::new();
        splitter.split(processes_info);
        splitter.display();
        splitter.collect_processes()
    } else {
        processes_info
    };

    let processes_info: Vec<ProcessInfo> = if let Some(var) = cli.split_env {
        let mut splitter = ProcessSplitterByEnvVariable::new(var);
        splitter.split(processes_info);
        splitter.display();
        splitter.collect_processes()
    } else {
        processes_info
    };

    if !cli.split_pids.is_empty() {
        let mut splitter = ProcessSplitterByPids::new(&cli.split_pids);
        splitter.split(processes_info);
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
    info!("rssanon = {rssanon}");
    info!("vmrss = {vmrss}");
    info!("global_elapsed = {global_elapsed:?}");
}

#![allow(dead_code)]
#![allow(unused_variables)]
#![feature(drain_filter)]

// TODO:
// - remove unwraps
//

use itertools::Itertools;
use procfs::{
    process::{PageInfo, Pfn, Process},
    PhysicalPageFlags,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt,
    process::Command,
};

struct ProcessInfo {
    process: Process,
    pfns: HashSet<Pfn>,
    swap_pages: HashSet<(u64, u64)>,
    rss: u64,
    vsz: u64,
    pte: u64,
    fds: usize,
}

struct ProcessGroupInfo {
    name: String,
    processes_info: Vec<ProcessInfo>,
    pfns: HashSet<Pfn>,
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
    let mut pfns: HashSet<Pfn> = HashSet::new();
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
        vsz += memory_map.address.1 - memory_map.address.0;

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

    Ok(ProcessInfo {
        process,
        pfns,
        swap_pages,
        rss,
        vsz,
        pte,
        fds,
    })
}

trait ProcessSplitter<'a> {
    type GroupIter<'b: 'a>: Iterator<Item = &'a ProcessGroupInfo>
    where
        Self: 'b;
    fn split(&mut self, processes: Vec<ProcessInfo>);
    fn iter_groups<'b>(&'b self) -> Self::GroupIter<'b>;
    fn collect_processes(self) -> Vec<ProcessInfo>;
}

struct ProcessSplitterByEnvVariable {
    var: OsString,
    groups: HashMap<Option<OsString>, ProcessGroupInfo>,
}
impl ProcessSplitterByEnvVariable {
    fn new<S: AsRef<OsStr>>(var: S) -> Self {
        Self {
            groups: HashMap::new(),
            var: var.as_ref().to_os_string(),
        }
    }
}

impl<'a> ProcessSplitter<'a> for ProcessSplitterByEnvVariable {
    type GroupIter<'b: 'a> =
        std::collections::hash_map::Values<'a, Option<OsString>, ProcessGroupInfo>;

    fn split(&mut self, mut processes: Vec<ProcessInfo>) {
        let sids: HashSet<Option<OsString>> = processes
            .iter()
            .filter_map(|p| p.process.environ().map(|x| x.get(&self.var).cloned()).ok())
            .collect();

        let mut groups: HashMap<Option<OsString>, ProcessGroupInfo> = HashMap::new();
        for sid in sids {
            let some_processes: Vec<ProcessInfo> = processes
                .drain_filter(|p| match p.process.environ().ok() {
                    Some(env) => env.get(&self.var) == sid.as_ref(),
                    None => false,
                })
                .collect();
            let name = format!("{:?}={:?}", self.var, sid);
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
struct ProcessSplitterByPids {
    pids: Vec<i32>,
    groups: BTreeMap<u8, ProcessGroupInfo>,
}

impl ProcessSplitterByPids {
    fn new(pids: &[i32]) -> Self {
        Self {
            pids: pids.to_vec(),
            groups: BTreeMap::new(),
        }
    }
}
impl<'a> ProcessSplitter<'a> for ProcessSplitterByPids {
    type GroupIter<'b: 'a> = std::collections::btree_map::Values<'a, u8, ProcessGroupInfo>;
    fn split(&mut self, processes: Vec<ProcessInfo>) {
        let mut processes_info_0 = Vec::new();
        let mut processes_info_1 = Vec::new();

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
struct ProcessSplitterByUid {
    groups: BTreeMap<u32, ProcessGroupInfo>,
}

impl ProcessSplitterByUid {
    fn new() -> Self {
        Self {
            groups: BTreeMap::new(),
        }
    }
}
impl<'a> ProcessSplitter<'a> for ProcessSplitterByUid {
    type GroupIter<'b: 'a> = std::collections::btree_map::Values<'a, u32, ProcessGroupInfo>;
    fn split(&mut self, mut processes_info: Vec<ProcessInfo>) {
        let uids: HashSet<u32> = processes_info
            .iter()
            .filter_map(|p| p.process.uid().ok())
            .collect();

        for uid in uids {
            let username = users::get_user_by_uid(uid);
            let username = match username {
                Some(username) => username.name().to_string_lossy().to_string(),
                None => format!("{uid}"),
            };
            let processes_info: Vec<ProcessInfo> = processes_info
                .drain_filter(|p| p.process.uid().ok() == Some(uid))
                .collect();
            let name = format!("user {}", username);
            let group_info = processes_group_info(processes_info, name);
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

fn processes_group_info(processes_info: Vec<ProcessInfo>, name: String) -> ProcessGroupInfo {
    let mut pfns = HashSet::new();
    let mut swap_pages = HashSet::new();
    let mut pte = 0;
    let mut fds = 0;

    for process_info in &processes_info {
        pfns.extend(&process_info.pfns);
        swap_pages.extend(&process_info.swap_pages);
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

/// Spawn new process with different user
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
        .arg("get_sga")
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
    let args: Vec<String> = std::env::args().collect();

    if args.get(1) == Some(&String::from("get_sga")) {
        assert_ne!(users::get_effective_uid(), 0);

        // subprogram to connect to instance and print sga size
        // We should have the correct context (user, env vars) to connect to database
        let sga_size = snap::get_sga_size().unwrap();

        // print value
        // parent will grab that value in `get_smon_info`
        println!("{sga_size}");
        std::process::exit(0);
    }

    assert_eq!(users::get_effective_uid(), 0);

    let pids: Vec<i32> = args
        .iter()
        .skip(1)
        .map(|s| s.parse().expect("PID arg must be a number"))
        .collect();

    // first run
    // find smons processes, and for each spawn a new process in the correct context to get infos

    let instances: Vec<SmonInfo> = snap::find_smons()
        .iter()
        .filter_map(|(pid, uid, sid, home)| {
            let smon_info = get_smon_info(*pid, *uid, sid.as_os_str(), home.as_os_str());

            smon_info.ok()
        })
        .collect();

    if !instances.is_empty() {
        println!("Oracle instances:");
        for instance in &instances {
            println!("{:?} sga={}B", instance.sid, instance.sga_size);
        }
    }
    println!();

    let page_size = procfs::page_size();

    // shm (key, id) -> PFNs
    let mut shm_pfns: HashMap<(i32, u64), HashSet<Pfn>> = HashMap::new();
    for shm in procfs::Shm::new().expect("Can't read /dev/sysvipc/shm") {
        let pfns = snap::shm2pfns(&shm).unwrap();
        shm_pfns.insert((shm.key, shm.shmid), pfns);
    }

    // probably incorrect?
    // size of kernel structures
    //let current_kernel = procfs::sys::kernel::Version::current().unwrap();
    //let (fd_size, task_size) =
    //    snap::get_kernel_datastructure_size(current_kernel).expect("Unknown kernel");

    //let mut kpagecount = procfs::KPageCount::new().expect("Can't open /proc/kpagecount");
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

    let chrono = std::time::Instant::now();
    let processes: Vec<ProcessInfo> = procfs::process::all_processes()
        .unwrap()
        .filter_map(|proc| proc.ok())
        .filter_map(|process| get_info(process).ok())
        .collect();
    println!("Scanned processes: {:?}", chrono.elapsed());

    users::get_current_uid();

    println!();

    let mut splitter = ProcessSplitterByUid::new();
    {
        let chrono = std::time::Instant::now();
        splitter.split(processes);
        println!("Processes by user:");
        for group_1 in splitter.iter_groups() {
            let mut other_pfns = HashSet::new();
            for group_2 in splitter.iter_groups() {
                if group_1 != group_2 {
                    other_pfns.extend(&group_2.pfns);
                }
            }

            let pfns = group_1.pfns.len();
            let rss = group_1.pfns.len() as u64 * page_size / 1024 / 1024;
            let uss = group_1.pfns.difference(&other_pfns).count() as u64 * page_size / 1024 / 1024;

            println!("{:<30} RSS {:>6} MiB USS {:>6} MiB", group_1.name, rss, uss);
        }
        println!("Split by uid: {:?}", chrono.elapsed());
        println!();
    }

    // get processes back, consuming `groups`
    let processes: Vec<ProcessInfo> = splitter.collect_processes();

    let mut splitter = ProcessSplitterByEnvVariable::new("ORACLE_SID");
    println!("Processes by env variable 'ORACLE_SID'");
    {
        let chrono = std::time::Instant::now();
        splitter.split(processes);
        for group1_info in splitter.iter_groups() {
            let mut other_pfns = HashSet::new();
            for group2_info in splitter.iter_groups() {
                if group1_info != group2_info {
                    other_pfns.extend(&group2_info.pfns);
                }
            }

            let pfns = group1_info.pfns.len();
            let rss = group1_info.pfns.len() as u64 * page_size / 1024 / 1024;
            let uss =
                group1_info.pfns.difference(&other_pfns).count() as u64 * page_size / 1024 / 1024;

            println!(
                "{:<30} RSS {:>6} MiB USS {:>6} MiB",
                group1_info.name, rss, uss
            );
        }
        println!("Split by uid: {:?}", chrono.elapsed());
        println!();
    }

    // get processes back, consuming `groups`
    let processes: Vec<ProcessInfo> = splitter.collect_processes();

    {
        if !pids.is_empty() {
            let mut splitter = ProcessSplitterByPids::new(&pids);
            println!("Processes by PIDs");
            let chrono = std::time::Instant::now();
            splitter.split(processes);
            for group_1 in splitter.iter_groups() {
                let mut other_pfns = HashSet::new();
                for group_2 in splitter.iter_groups() {
                    if group_1 != group_2 {
                        other_pfns.extend(&group_2.pfns);
                    }
                }

                let pfns = group_1.pfns.len();
                let rss = group_1.pfns.len() as u64 * page_size / 1024 / 1024;
                let uss =
                    group_1.pfns.difference(&other_pfns).count() as u64 * page_size / 1024 / 1024;

                println!("{}\nRSS {:>6} MiB USS {:>6} MiB", group_1.name, rss, uss);
            }
            println!("Split by uid: {:?}", chrono.elapsed());
            println!();
        }
    }
}

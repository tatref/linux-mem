#![allow(dead_code)]
#![allow(unused_variables)]
#![feature(drain_filter)]

// TODO:
// - args for splits
//   - oracle: --oracle
//   - shm: --shm
//   - uid: --uid
//   - pids: --pids 1 2 3
//   - env: --env ORACLE_SID
// - filters
// - remove unwraps

use itertools::Itertools;
use procfs::{
    process::{PageInfo, Pfn, Process},
    PhysicalPageFlags, Shm,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::{OsStr, OsString},
    io::{stdout, Write},
    os::unix::process::CommandExt,
    process::Command,
};

type PfnHashSet = HashSet<Pfn>;

struct ProcessInfo {
    process: Process,
    uid: u32,
    environ: HashMap<OsString, OsString>,
    pfns: PfnHashSet,
    swap_pages: HashSet<(u64, u64)>,
    rss: u64,
    vsz: u64,
    pte: u64,
    fds: usize,
}

struct ProcessGroupInfo {
    name: String,
    processes_info: Vec<ProcessInfo>,
    pfns: PfnHashSet,
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
    let mut pfns: PfnHashSet = HashSet::new();
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

trait ProcessSplitter<'a> {
    fn name(&self) -> String;
    type GroupIter<'b: 'a>: Iterator<Item = &'a ProcessGroupInfo>
    where
        Self: 'b;
    fn split(&mut self, shms: &HashMap<Shm, HashSet<Pfn>>, processes: Vec<ProcessInfo>);
    fn iter_groups<'b>(&'b self) -> Self::GroupIter<'b>;
    fn collect_processes(self) -> Vec<ProcessInfo>;

    fn display(&'a self) {
        let chrono = std::time::Instant::now();
        println!("Process groups by {}", self.name());
        println!("{:<30} {:>6} MiB {:>6} MiB", "group_name", "RSS", "USS");
        println!("====================================================");
        for group_1 in self.iter_groups() {
            let mut other_pfns = HashSet::new();
            for group_2 in self.iter_groups() {
                if group_1 != group_2 {
                    other_pfns.extend(&group_2.pfns);
                }
            }

            let rss = group_1.pfns.len() as u64 * procfs::page_size() / 1024 / 1024;
            let uss = group_1.pfns.difference(&other_pfns).count() as u64 * procfs::page_size()
                / 1024
                / 1024;

            println!("{:<30} {:>10} {:>10}", group_1.name, rss, uss);
        }
        println!("\nSplit by {}: {:?}", self.name(), chrono.elapsed());
        println!();
    }
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

    fn name(&self) -> String {
        format!("environment variable {}", self.var.to_string_lossy())
    }
    fn split(&mut self, shms: &HashMap<Shm, HashSet<Pfn>>, mut processes: Vec<ProcessInfo>) {
        let sids: HashSet<Option<OsString>> = processes
            .iter()
            .map(|p| p.environ.get(&self.var).cloned())
            .collect();

        let mut groups: HashMap<Option<OsString>, ProcessGroupInfo> = HashMap::new();
        for sid in sids {
            let some_processes: Vec<ProcessInfo> = processes
                .drain_filter(|p| p.environ.get(&self.var) == sid.as_ref())
                .collect();
            let name = format!(
                "{}={:?}",
                self.var.to_string_lossy(),
                sid.as_ref().map(|os| os.to_string_lossy().to_string())
            );
            let process_group_info = processes_group_info(shms, some_processes, name);
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

    fn name(&self) -> String {
        format!("PID list")
    }
    fn split(&mut self, shms: &HashMap<Shm, HashSet<Pfn>>, processes: Vec<ProcessInfo>) {
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
        let process_group_info_0 = processes_group_info(shms, processes_info_0, name_0);
        let process_group_info_1 = processes_group_info(shms, processes_info_1, name_1);

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

    fn name(&self) -> String {
        format!("UID")
    }
    fn split(&mut self, shms: &HashMap<Shm, HashSet<Pfn>>, mut processes: Vec<ProcessInfo>) {
        let uids: HashSet<u32> = processes.iter().map(|p| p.uid).collect();

        for uid in uids {
            let username = users::get_user_by_uid(uid);
            let username = match username {
                Some(username) => username.name().to_string_lossy().to_string(),
                None => format!("{uid}"),
            };
            let processes_info: Vec<ProcessInfo> =
                processes.drain_filter(|p| p.uid == uid).collect();
            let group_info = processes_group_info(shms, processes_info, username);
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

fn processes_group_info(
    shms: &HashMap<Shm, HashSet<Pfn>>,
    processes_info: Vec<ProcessInfo>,
    name: String,
) -> ProcessGroupInfo {
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

    if users::get_effective_uid() != 0 {
        panic!("Run as root");
    }

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
        println!("SID               SGA MiB");
        println!("==========================");
        for instance in &instances {
            println!(
                "{:<12} {:>12}",
                instance.sid.to_string_lossy(),
                instance.sga_size / 1024 / 1024
            );
        }
        println!();
    } else {
        println!("Can't locate any Oracle instance");
    }

    let page_size = procfs::page_size();

    // shm (key, id) -> PFNs
    let mut shms: HashMap<procfs::Shm, HashSet<Pfn>> = HashMap::new();
    for shm in procfs::Shm::new().expect("Can't read /dev/sysvipc/shm") {
        let pfns = snap::shm2pfns(&shm).unwrap();
        shms.insert(shm, pfns);
    }

    if !shms.is_empty() {
        println!("Shared memory segments:");
        println!("         key           id       PFNs    RSS MiB",);
        println!("===============================================",);
        for (shm, pfns) in &shms {
            println!(
                "{:>12} {:>12} {:>10} {:>10}",
                shm.key,
                shm.shmid,
                pfns.len(),
                pfns.len() * page_size as usize / 1024 / 1024
            );
        }
        println!();
    } else {
        println!("Can't locate any shared memory segment")
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

    let my_pid = std::process::id();

    println!("Scanning processes...");
    let chrono = std::time::Instant::now();
    let all_processes: Vec<_> = procfs::process::all_processes().unwrap().collect();
    let processes_count = all_processes.len();
    let mut processes_info = Vec::new();
    for (idx, proc) in all_processes.into_iter().enumerate() {
        if idx % 10 == 0 {
            print!("{}/{}\r", idx, processes_count);
            let _ = stdout().lock().flush();
        }
        let Ok(proc) = proc else {continue;};
        if proc.pid as u32 != my_pid {
            let Ok(info) = get_info(proc) else {continue;};
            processes_info.push(info);
        }
    }
    println!();
    println!(
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
    println!(
        "Total PFNs: {total_pfns} ({} MiB)",
        total_pfns * page_size as usize / 1024 / 1024
    );
    println!(
        "Max PFNs: {max_pfns} ({} MiB)",
        max_pfns * page_size as usize / 1024 / 1024
    );
    println!();

    let mut splitter = ProcessSplitterByUid::new();
    splitter.split(&shms, processes_info);
    splitter.display();
    let processes: Vec<ProcessInfo> = splitter.collect_processes();

    let mut splitter = ProcessSplitterByEnvVariable::new("ORACLE_SID");
    splitter.split(&shms, processes);
    splitter.display();
    let processes: Vec<ProcessInfo> = splitter.collect_processes();

    if !pids.is_empty() {
        let mut splitter = ProcessSplitterByPids::new(&pids);
        splitter.split(&shms, processes);
        splitter.display();
    }
}

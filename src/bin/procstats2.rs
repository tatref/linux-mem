#![allow(dead_code)]
#![allow(unused_variables)]
#![feature(drain_filter)]

// TODO:
// - remove unwraps
//

use procfs::{
    process::{MemoryMap, PageInfo, Pfn, Process},
    PhysicalPageFlags, Shm,
};
use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt,
    process::Command,
};

struct ProcessInfo {
    pid: i32,
    pfns: HashSet<Pfn>,
    swap_pages: HashSet<(u64, u64)>,
    rss: u64,
    vsz: u64,
    pte: u64,
    fds: usize,
}

struct ProcessGroupInfo {
    pids: Vec<i32>,
    pfns: HashSet<Pfn>,
    swap_pages: HashSet<(u64, u64)>,
    pte: u64,
    fds: usize,
}

struct SmonInfo {
    pid: i32,
    sid: OsString,
    sga_size: u64,
    sga_shm: Shm,
    sga_pfns: HashSet<Pfn>,
}

// return info memory maps info for standard process or None for kernel process
fn get_info(process: &Process, memory_maps: &[(MemoryMap, Vec<PageInfo>)]) -> Option<ProcessInfo> {
    if process.cmdline().unwrap().is_empty() {
        return None;
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
    let pte = process.status().unwrap().vmpte.unwrap();

    // file descriptors
    let fds = process.fd_count().unwrap();

    for (memory_map, pages) in memory_maps.iter() {
        //println!("{memory_map:?}");
        //println!(
        //    "{} pages",
        //    (memory_map.address.1 - memory_map.address.0) / page_size
        //);

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

    Some(ProcessInfo {
        pid: process.pid,
        pfns,
        swap_pages,
        rss,
        vsz,
        pte,
        fds,
    })
}

struct ProcessGroup {
    name: String,
    processes: Vec<Process>,
}
struct ProcessSplitterBySID {
    groups: HashMap<Option<OsString>, ProcessGroup>,
}
impl ProcessSplitterBySID {
    fn split(mut processes: Vec<Process>) -> Self {
        let sids: HashSet<Option<OsString>> = processes
            .iter()
            .map(|p| {
                let environ = p.environ().unwrap();
                environ.get(&OsString::from("ORACLE_SID")).cloned()
            })
            .collect();

        let mut groups = HashMap::new();
        for sid in sids {
            let some_processes: Vec<Process> = processes
                .drain_filter(|p| {
                    p.environ().unwrap().get(&OsString::from("ORACLE_SID")) == sid.as_ref()
                })
                .collect();
            let process_group = ProcessGroup {
                name: format!("SID={:?}", sid),
                processes: some_processes,
            };
            groups.insert(sid, process_group);
        }
        Self { groups }
    }
    fn groups(&self) -> impl Iterator<Item = &ProcessGroup> {
        self.groups.values()
    }
    fn collect_processes(self) -> Vec<Process> {
        self.groups
            .into_values()
            .flat_map(|group| group.processes)
            .collect()
    }
}
struct ProcessSplitterByUid {
    groups: HashMap<u32, ProcessGroup>,
}
impl ProcessSplitterByUid {
    fn split(mut processes: Vec<Process>) -> Self {
        let uids: HashSet<u32> = processes.iter().map(|p| p.uid().unwrap()).collect();
        let mut groups = HashMap::new();

        for uid in uids {
            let username = users::get_user_by_uid(uid).unwrap();
            let username = username.name().to_string_lossy();
            let some_processes: Vec<Process> = processes
                .drain_filter(|p| p.uid().unwrap() == uid)
                .collect();
            let process_group = ProcessGroup {
                name: format!("{}'s processes", username),
                processes: some_processes,
            };
            groups.insert(uid, process_group);
        }
        Self { groups }
    }
    fn groups(&self) -> impl Iterator<Item = &ProcessGroup> {
        self.groups.values()
    }
    fn collect_processes(self) -> Vec<Process> {
        self.groups
            .into_values()
            .flat_map(|group| group.processes)
            .collect()
    }
}

fn processes_group_info(group: &ProcessGroup) -> ProcessGroupInfo {
    let processes_info: Vec<ProcessInfo> = group
        .processes
        .iter()
        .filter_map(|p| {
            let memory_maps = match snap::get_memory_maps_for_process(&p) {
                Ok(x) => x,
                Err(e) => {
                    return None;
                }
            };

            Some((p, memory_maps))
        })
        .filter_map(|(process, memory_info)| get_info(process, &memory_info))
        .collect();

    let mut pids = Vec::new();
    let mut pfns = HashSet::new();
    let mut swap_pages = HashSet::new();
    let mut pte = 0;
    let mut fds = 0;

    for process_info in processes_info.iter() {
        pids.push(process_info.pid);
        pfns.extend(&process_info.pfns);
        swap_pages.extend(&process_info.swap_pages);
        pte += process_info.pte;
        fds += process_info.fds;
    }

    ProcessGroupInfo {
        pids,
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
    let myself = std::env::args().nth(0).unwrap();

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

    let (sga_shm, sga_pfns) = procfs::Shm::new()?
        .iter()
        .filter(|shm| shm.size as u64 == sga_size)
        .map(|shm| (shm.clone(), snap::shm2pfns(shm).unwrap()))
        .next()
        .expect("shm for sga not found");

    let result = SmonInfo {
        pid,
        sga_pfns,
        sga_shm,
        sga_size,
        sid: sid.to_os_string(),
    };

    Ok(result)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().nth(1) == Some(&String::from("get_sga")) {
        assert_ne!(users::get_current_uid(), 0);

        // subprogram to connect to instance and print sga size
        // We should have the correct context (user, env vars) to connect to database

        let sga_size = snap::get_sga_size();
        println!("{sga_size}");
        std::process::exit(0);
    }

    assert_eq!(users::get_current_uid(), 0);

    // first run
    // find smons processes, and for each spawn a new process in the correct context

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

    // the PIDs we want to mesure
    let pids: Vec<i32> = std::env::args()
        .skip(1)
        .map(|pid| pid.parse().expect("Not a number"))
        .collect();

    if pids.len() < 1 {
        panic!("Provide at least 1 PID");
    }

    // shm (key, id) -> PFNs
    let mut shm_pfns: HashMap<(i32, u64), HashSet<Pfn>> = HashMap::new();
    for shm in procfs::Shm::new().expect("Can't read /dev/sysvipc/shm") {
        let pfns = snap::shm2pfns(&shm).unwrap();
        shm_pfns.insert((shm.key, shm.shmid), pfns);
    }

    // size of kernel structures
    let current_kernel = procfs::sys::kernel::Version::current().unwrap();
    let (fd_size, task_size) =
        snap::get_kernel_datastructure_size(current_kernel).expect("Unknown kernel");

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

    let my_pid = std::process::id();

    let processes: Vec<Process> = procfs::process::all_processes()
        .unwrap()
        .filter_map(|res| res.ok())
        .filter(|p| p.pid != my_pid as i32)
        .collect();

    users::get_current_uid();

    let groups = ProcessSplitterByUid::split(processes);
    for group in groups.groups() {
        let group_info = processes_group_info(&group);

        let pfns = group_info.pfns.len();
        let rss = group_info.pfns.len() as u64 * page_size / 1024 / 1024;

        println!("{:<10} {} MiB", group.name, rss);
    }
    println!();

    let processes: Vec<Process> = groups.collect_processes();
    let groups = ProcessSplitterBySID::split(processes);
    for group in groups.groups() {
        let group_info = processes_group_info(&group);

        let pfns = group_info.pfns.len();
        let rss = group_info.pfns.len() as u64 * page_size / 1024 / 1024;

        println!("{:<10} {} MiB", group.name, rss);
    }
    println!();

    unreachable!();
    /*
    let my_processes_group_infos = processes_group_info(&my_pids);
    let other_processes_group_infos = processes_group_info(&other_pids);

    dbg!(chrono.elapsed());

    // stats
    let total_rss = my_processes_group_infos.pfns.len() as u64 * page_size;
    let other_rss = other_processes_group_infos.pfns.len() as u64 * page_size;

    let common_rss = my_processes_group_infos
        .pfns
        .intersection(&other_processes_group_infos.pfns)
        .count() as u64
        * page_size;

    let total_pte = my_processes_group_infos.pte;
    let other_pte = other_processes_group_infos.pte;

    let total_fds_size = fd_size * my_processes_group_infos.fds as u64;
    let total_tasks_size = task_size * my_processes_group_infos.pids.len() as u64;

    let grand_total = total_rss + total_pte + total_fds_size + total_tasks_size;

    println!(
        "other rss: {}",
        humansize::format_size(other_rss, humansize::BINARY)
    );

    println!(
        "common rss: {}",
        humansize::format_size(common_rss, humansize::BINARY)
    );

    println!(
        "total rss: {}",
        humansize::format_size(total_rss, humansize::BINARY)
    );

    println!(
        "other_pte: {}",
        humansize::format_size(other_pte * 1024, humansize::BINARY)
    );

    println!(
        "total_pte: {}",
        humansize::format_size(total_pte * 1024, humansize::BINARY)
    );

    println!(
        "total_fds_size: {}",
        humansize::format_size(total_fds_size, humansize::BINARY)
    );

    println!(
        "total_task_struct_size: {}",
        humansize::format_size(total_tasks_size, humansize::BINARY)
    );

    println!(
        "Grand total: {}",
        humansize::format_size(grand_total, humansize::BINARY)
    );
    */
}

#![cfg_attr(target_os = "linux", feature(setgroups))]
#![allow(non_snake_case)]

// https://biriukov.dev/docs/page-cache/4-page-cache-eviction-and-page-reclaim/
// cat /proc/$(pidof cat)/smaps_rollup
// cat /proc/$(pidof cat)/status
// pmap -X $(pidof cat)
// smem --processfilter=cat
// pahole -C task_struct /sys/kernel/btf/vmlinux

use itertools::Itertools;

#[cfg(unix)]
use procfs::{
    page_size,
    process::{MMapPath, Process},
    process::{MemoryMap, PageInfo},
    Shm,
};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use procfs_core::{
    process::Pfn, ExplicitSystemInfo, PhysicalMemoryMap, PhysicalPageFlags, WithSystemInfo,
};

use rayon::prelude::ParallelExtend;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fmt::{Debug, Display},
    hash::BuildHasherDefault,
    process::{Command, Stdio},
    str::FromStr,
};

use log::{info, warn};

use oracle::{Connector, Privilege};
use std::ffi::OsString;

#[cfg(unix)]
pub mod filters;
#[cfg(unix)]
pub mod groups;
#[cfg(unix)]
pub mod process_tree;
#[cfg(unix)]
pub mod tmpfs;

/// Convert pfn to index into non-contiguous memory mappings
pub fn pfn_to_index(iomem: &[PhysicalMemoryMap], page_size: u64, pfn: Pfn) -> Option<u64> {
    if pfn.0 == 0 {
        return None;
    }

    let mut previous_maps_size = 0;
    for map in iomem {
        assert_eq!(map.name, "System RAM");
        let (pfn_start, pfn_end) = (
            Pfn(map.address.0 / page_size),
            Pfn(map.address.1 / page_size),
        );
        if pfn < pfn_start {
            return None;
        }
        if pfn <= pfn_end {
            return Some(previous_maps_size + pfn.0 - pfn_start.0);
        }
        previous_maps_size += pfn_end.0 - pfn_start.0 + 1;
    }
    None
}

/// Convert index to Pfn into non-contiguous memory mappings
pub fn index_to_pfn(iomem: &[PhysicalMemoryMap], page_size: u64, mut index: u64) -> Option<Pfn> {
    for map in iomem {
        assert_eq!(map.name, "System RAM");
        let (pfn_start, pfn_end) = (
            Pfn(map.address.0 / page_size),
            Pfn(map.address.1 / page_size),
        );
        if index <= pfn_end.0 - pfn_start.0 {
            return Some(Pfn(index + pfn_start.0));
        }
        index -= pfn_end.0 - pfn_start.0 + 1;
    }
    None
}

/// Return None if map name is not "System RAM"
/// Return Some(idx) if map name is "System RAM". idx is the index of the particular mapping, counting only RAM mappings
pub fn pfn_is_in_ram(iomem: &[PhysicalMemoryMap], page_size: u64, pfn: Pfn) -> Option<usize> {
    for (idx, memory_map) in iomem
        .iter()
        .filter(|map| map.name == "System RAM")
        .enumerate()
    {
        if pfn.0 * page_size >= memory_map.address.0 && pfn.0 * page_size < memory_map.address.1 {
            return Some(idx);
        }
    }

    None
}

/// Count total number of 4 kiB frames in memory segments
pub fn get_pfn_count(iomem: &[PhysicalMemoryMap]) -> u64 {
    iomem
        .iter()
        .map(|map| {
            //let (start, end) = map.get_range().get();
            //(map.address.1 - map.address.0) / 4096
            let system_info = ExplicitSystemInfo {
                boot_time_secs: 0,
                is_little_endian: false,
                page_size: 4096,
                ticks_per_second: 1000,
            };
            let (start, end) = map.get_range().with_system_info(&system_info);
            end.0 - start.0
        })
        .sum()
}

/// Get size of memory mapping in bytes
pub fn get_size(map: &PhysicalMemoryMap) -> u64 {
    map.address.1 - map.address.0
}

pub const FLAG_NAMES: [&str; 27] = [
    "LOCKED",
    "ERROR",
    "REFERENCED",
    "UPTODATE",
    "DIRTY",
    "LRU",
    "ACTIVE",
    "SLAB",
    "WRITEBACK",
    "RECLAIM",
    "BUDDY",
    "MMAP",
    "ANON",
    "SWAPCACHE",
    "SWAPBACKED",
    "COMPOUND_HEAD",
    "COMPOUND_TAIL",
    "HUGE",
    "UNEVICTABLE",
    "HWPOISON",
    "NOPAGE",
    "KSM",
    "THP",
    "OFFLINE",
    "ZERO_PAGE",
    "IDLE",
    "PGTABLE",
];

pub fn compute_compound_pages(data: &[PhysicalPageFlags]) -> [u64; FLAG_NAMES.len() + 1] {
    let mut counters = [0u64; FLAG_NAMES.len() + 1];

    #[allow(unused_variables)]
    let mut merged_compound_pages = 0;
    let mut iter = data.iter().peekable();
    while let Some(&flags) = iter.next() {
        if flags.contains(PhysicalPageFlags::COMPOUND_HEAD) {
            let head_flags = flags;
            //println!("0: {:?}", head_flags);

            for (index, _) in FLAG_NAMES.iter().enumerate() {
                if head_flags.bits() & (1 << index) == 1 << index {
                    counters[index] += 1;
                }
            }

            for &flags in
                iter.take_while_ref(|flags| flags.contains(PhysicalPageFlags::COMPOUND_TAIL))
            {
                merged_compound_pages += 1;
                let mut tail_flags = flags;
                tail_flags.insert(head_flags & !PhysicalPageFlags::COMPOUND_HEAD);

                //println!("head: {:?} tail: {:?}", head_flags, tail_flags);

                for (index, _) in FLAG_NAMES.iter().enumerate() {
                    if tail_flags.bits() & (1 << index) == 1 << index {
                        counters[index] += 1;
                    }
                }
                continue;
            }
        } else {
            // no COMPOUND_HEAD, no COMPOUND_TAIL (except if bug)
            assert!(!flags.contains(PhysicalPageFlags::COMPOUND_TAIL));

            for (index, _) in FLAG_NAMES.iter().enumerate() {
                if flags.bits() & (1 << index) == 1 << index {
                    counters[index] += 1;
                }
            }
        }
    }

    //dbg!(merged_compound_pages);

    counters
}

/// Scan each page of shm
/// Return None if shm uses any swap
#[cfg(unix)]
pub fn shm2pfns(
    all_physical_pages: &HashMap<Pfn, PhysicalPageFlags>,
    shm: &Shm,
    force_read: bool,
) -> Result<Option<(HashSet<Pfn>, HashSet<(u64, u64)>, usize, usize)>, Box<dyn std::error::Error>> {
    let ptr: *mut libc::c_void;
    let shmid: libc::c_int = shm.shmid as i32;
    let must_read = shm.swap == 0 || force_read;

    // Map shared memory to current process
    {
        let shmaddr: *const libc::c_void = core::ptr::null();
        let shmflags: libc::c_int = libc::SHM_RDONLY;

        unsafe {
            ptr = libc::shmat(shmid, shmaddr, shmflags);
            if ptr == -1i32 as *mut libc::c_void {
                println!("shmat failed for shmid {shmid}");
                return Err(std::io::Error::last_os_error().into());
            }

            // try to read the shm
            // don't read if shm uses swap, as this would load the whole mapping into RAM
            let ptr = ptr as *mut u8;
            let mut dummy = 0;

            // only read if shm is not in swap
            if must_read {
                // we must read each page to populate pagemap
                let slice = std::slice::from_raw_parts_mut(ptr, shm.size as usize);
                for val in slice.iter().step_by(page_size() as usize) {
                    dummy += *val;
                }
            } else {
                warn!(
                    "Skipping read for shm key:{} id:{} because it uses swap",
                    shm.key, shm.shmid
                );
            }
            // prevent optimization
            std::hint::black_box(dummy);
        }
    }

    // walk virtual addresses
    let me = procfs::process::Process::myself()?;
    let mut pagemap = me.pagemap()?;
    let maps = me.maps()?;

    let map: &MemoryMap = maps
        .iter()
        .find(|map| map.address.0 == ptr as u64)
        .ok_or("Map not found")?; // return if shared memory is not found

    let (start, end) = (
        map.address.0 / procfs::page_size(),
        map.address.1 / procfs::page_size(),
    );

    let mut pfns = HashSet::new();
    let mut swap_pages = HashSet::new();
    for page_info in pagemap.get_range_info((start as usize)..(end as usize))? {
        match page_info {
            PageInfo::MemoryPage(mem_page) => {
                let pfn = mem_page.get_page_frame_number();
                pfns.insert(pfn);
            }
            PageInfo::SwapPage(swap_page) => {
                let swap_type = swap_page.get_swap_type();
                let swap_offset = swap_page.get_swap_offset();
                swap_pages.insert((swap_type, swap_offset));
            }
        }
    }

    let mut total_pages = 0;
    let mut huge_pages = 0;
    for pfn in &pfns {
        let flags = match all_physical_pages.get(pfn) {
            Some(x) => x,
            None => continue, // page is not in RAM (in swap, or we didn't read that page, so Linux didn't create a memory mapping
        };
        total_pages += 1;
        if flags.contains(PhysicalPageFlags::HUGE) {
            // the doc states that HUGE flag is set only on HEAD pages, but seems like it also set on TAIL pages
            huge_pages += 1;
        }
    }
    let pages_4k = total_pages - huge_pages;
    let pages_2M = huge_pages / 512;

    // detach shm
    unsafe {
        let ret = libc::shmdt(ptr);
        if ret != 0 {
            println!("shmdt failed for shmid {shmid}");
            return Err(std::io::Error::last_os_error().into());
        }
    }

    if must_read {
        Ok(Some((pfns, swap_pages, pages_4k, pages_2M)))
    } else {
        Ok(None)
    }
}

/// Return size of (files_struct, task_struct) from kernel
/// ./pahole -C files_struct /sys/kernel/btf/vmlinux
/// ./pahole -C task_struct /sys/kernel/btf/vmlinux
#[cfg(unix)]
pub fn get_kernel_datastructure_size(
    current_kernel: procfs::sys::kernel::Version,
) -> Option<(u64, u64)> {
    let mut kernel_struct_sizes: HashMap<procfs::sys::kernel::Version, (u64, u64)> = HashMap::new();

    // OEL 6
    // 4.1.12
    // 3.8.13
    // 2.6.39
    // 2.6.32

    // OEL 7
    // 4.14.35
    // 4.1.12
    // 3.8.13

    // OEL 8
    let kernel = procfs::sys::kernel::Version::new(5, 4, 17);
    kernel_struct_sizes.insert(kernel, (704, 9408));

    // OEL 9
    let kernel = procfs::sys::kernel::Version::new(5, 15, 0);
    kernel_struct_sizes.insert(kernel, (704, 9856));

    kernel_struct_sizes.get(&current_kernel).copied()
}

/// If optimize_shm if true, only return first 10 pages for a shared memory mapping
#[cfg(unix)]
pub fn get_memory_maps_for_process(
    process: &Process,
    optimize_shm: bool,
) -> Result<Vec<(MemoryMap, Vec<PageInfo>)>, Box<dyn std::error::Error>> {
    let page_size = procfs::page_size();

    let mut pagemap = process.pagemap()?;
    let memmap = process.maps()?;

    let result = memmap
        .iter()
        .filter_map(|memory_map| {
            let index_start = (memory_map.address.0 / page_size) as usize;
            let index_end = (memory_map.address.1 / page_size) as usize;

            // can't scan Vsyscall, so skip it
            if memory_map.pathname == MMapPath::Vsyscall {
                return None;
            }

            if let (MMapPath::Vsys(_), true) = (&memory_map.pathname, optimize_shm) {
                return Some((memory_map.clone(), Vec::new()));
            }

            let pages = match pagemap.get_range_info(index_start..index_end) {
                Ok(x) => x,
                Err(_) => return None,
            };

            Some((memory_map.clone(), pages))
        })
        .collect();

    Ok(result)
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
pub enum LargePages {
    True,
    False,
    Only,
    AutoOnly,
}

impl Display for LargePages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl FromStr for LargePages {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "TRUE" => Ok(LargePages::True),
            "FALSE" => Ok(LargePages::False),
            "ONLY" => Ok(LargePages::Only),
            "AUTO_ONLY" => Ok(LargePages::AutoOnly),
            _ => Err(format!("Can't parse {:?} as LargePage value", s)),
        }
    }
}

/// Connect to DB using OS auth and env vars
/// return size of SGA
pub fn get_db_info() -> Result<(u64, u64, u64, LargePages), Box<dyn std::error::Error>> {
    let mut connector = Connector::new("", "", "");
    let mut connector = connector.external_auth(true);
    connector = if std::env::var("ORACLE_SID").unwrap().contains("+ASM") {
        connector.privilege(Privilege::Sysasm)
    } else {
        connector.privilege(Privilege::Sysdba)
    };
    let conn = connector.connect()?;
    let sql = "select sum(value) from v$sga where name in ('Variable Size', 'Database Buffers')";
    let sga_size = conn.query_row_as::<u64>(sql, &[])?;

    let sql = "select count(1), sum(pga_alloc_mem) from v$process";
    let (processes, pga) = conn.query_row_as::<(u64, u64)>(sql, &[])?;

    let sql = "select value from v$parameter where name = 'use_large_pages'";
    let large_pages: LargePages = conn.query_row_as::<String>(sql, &[])?.parse()?;

    Ok((sga_size, processes, pga, large_pages))
}

/// Find smons processes
/// For each, return (pid, uid, ORACLE_SID, ORACLE_HOME)
#[cfg(unix)]
pub fn find_smons() -> Vec<(i32, u32, OsString, OsString)> {
    let smons: Vec<Process> = procfs::process::all_processes()
        .unwrap()
        .filter_map(|proc| {
            let cmdline = proc.as_ref().ok()?.cmdline().ok()?;

            if cmdline.len() == 1
                && (cmdline[0].starts_with("ora_pmon_") || cmdline[0].starts_with("asm_pmon_"))
            {
                info!("Found smon {}", cmdline[0]);
                Some(proc.ok()?)
            } else {
                None
            }
        })
        .collect();

    let result = smons
        .iter()
        .filter_map(|smon| {
            let pid = smon.pid;
            let uid = smon.uid().ok()?;
            let environ = smon.environ().ok()?;
            let sid = environ.get(&OsString::from("ORACLE_SID"))?.to_os_string();
            let home = environ.get(&OsString::from("ORACLE_HOME"))?.to_os_string();

            Some((pid, uid, sid, home))
        })
        .collect();

    result
}

#[cfg(feature = "std")]
pub type TheHash = std::collections::hash_map::DefaultHasher;

#[cfg(feature = "fnv")]
pub type TheHash = fnv::FnvHasher;

#[cfg(feature = "ahash")]
pub type TheHash = ahash::AHasher;

#[cfg(feature = "metrohash")]
pub type TheHash = metrohash::MetroHash;

#[cfg(feature = "fxhash")]
pub type TheHash = rustc_hash::FxHasher;

#[cfg(unix)]
pub type ShmsMetadata = HashMap<
    procfs::Shm,
    Option<(HashSet<Pfn>, HashSet<(u64, u64)>, usize, usize)>,
    BuildHasherDefault<TheHash>,
>;

#[derive(Hash, Eq, PartialEq, Debug, Copy, Clone)]
pub struct ShmReference {
    key: i32,
    shmid: u64,
}

#[cfg(unix)]
pub struct ProcessInfo {
    pub process: Process,
    pub uid: u32,
    pub environ: HashMap<OsString, OsString>,
    pub pfns: HashSet<Pfn, BuildHasherDefault<TheHash>>,
    pub anon_pfns: HashSet<Pfn, BuildHasherDefault<TheHash>>,
    pub swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>>,
    pub anon_swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>>,
    pub referenced_shms: HashSet<Shm>,
    pub rss: u64,
    pub vsz: u64,
    pub pte: u64,
    pub fds: usize,
    pub unknown_shm: HashSet<ShmReference>,
}

#[cfg(unix)]
pub struct ProcessGroupInfo {
    pub name: String,
    pub processes_info: Vec<ProcessInfo>,
    pub pfns: HashSet<Pfn, BuildHasherDefault<TheHash>>,
    pub anon_pfns: HashSet<Pfn, BuildHasherDefault<TheHash>>,
    pub swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>>,
    pub anon_swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>>,
    pub referenced_shm: HashSet<Shm>,
    pub pte: u64,
    pub fds: usize,
}

#[cfg(unix)]
impl PartialEq for ProcessGroupInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

#[cfg(unix)]
impl Debug for ProcessGroupInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessGroupInfo")
            .field("name", &self.name)
            .field("processes", &self.processes_info.len())
            .field("pfns", &self.pfns.len())
            .field("swap_pages", &self.swap_pages.len())
            .field("referenced_shm", &self.referenced_shm)
            .field("pte", &self.pte)
            .field("fds", &self.fds)
            .finish()
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SmonInfo {
    pub pid: i32,
    pub sid: OsString,
    pub sga_size: u64,
    pub large_pages: LargePages,
    pub processes: u64,
    pub pga_size: u64,
    //sga_shm: Shm,
    //sga_pfns: HashSet<Pfn>,
}

// return info memory maps info for standard process or None for kernel process
#[cfg(unix)]
pub fn get_process_info(
    process: Process,
    shms_metadata: &ShmsMetadata,
) -> Result<ProcessInfo, Box<dyn std::error::Error>> {
    if process.cmdline()?.is_empty() {
        // already handled in main
        Err(String::from("No info for kernel process"))?
    }

    let page_size = procfs::page_size();

    // physical memory pages
    let mut pfns: HashSet<Pfn, BuildHasherDefault<TheHash>> = Default::default();
    let mut anon_pfns: HashSet<Pfn, BuildHasherDefault<TheHash>> = Default::default();
    // swap type, offset
    let mut swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>> = HashSet::default();
    let mut anon_swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>> = HashSet::default();

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

    let memory_maps = crate::get_memory_maps_for_process(&process, true)?;

    let mut referenced_shms = HashSet::new();

    let mut unknown_shm = HashSet::new();

    for (memory_map, pages) in memory_maps.iter() {
        let size = memory_map.address.1 - memory_map.address.0;
        vsz += size;
        let _max_pages = size / page_size;

        match &memory_map.pathname {
            MMapPath::Vsys(key) => {
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
                    unknown_shm.insert(ShmReference {
                        key: *key,
                        shmid: memory_map.inode,
                    });
                    warn!(
                        "Cant' find shm key {:?} shmid {:?} for pid {}",
                        key, memory_map.inode, process.pid
                    );
                }
            }
            MMapPath::Path(_) => {
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
            //MMapPath::Anonymous | MMapPath::Heap | MMapPath::Stack | MMapPath::TStack(_) => {
            _ => {
                // Count as "anon"
                for page in pages.iter() {
                    match page {
                        PageInfo::MemoryPage(memory_page) => {
                            let pfn = memory_page.get_page_frame_number();
                            if pfn.0 != 0 {
                                rss += page_size;
                            }
                            anon_pfns.insert(pfn);
                            pfns.insert(pfn);
                        }
                        PageInfo::SwapPage(swap_page) => {
                            let swap_type = swap_page.get_swap_type();
                            let offset = swap_page.get_swap_offset();

                            anon_swap_pages.insert((swap_type, offset));
                            swap_pages.insert((swap_type, offset));
                        }
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
        anon_pfns,
        referenced_shms,
        swap_pages,
        anon_swap_pages,
        rss,
        vsz,
        pte,
        fds,
        unknown_shm,
    })
}

#[cfg(unix)]
pub fn get_processes_group_info(
    processes_info: Vec<ProcessInfo>,
    name: &str,
    _shms_metadata: &ShmsMetadata,
) -> ProcessGroupInfo {
    let mut pfns: HashSet<Pfn, BuildHasherDefault<TheHash>> = HashSet::default();
    let mut anon_pfns: HashSet<Pfn, BuildHasherDefault<TheHash>> = HashSet::default();
    let mut swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>> = HashSet::default();
    let mut anon_swap_pages: HashSet<(u64, u64), BuildHasherDefault<TheHash>> = HashSet::default();
    let mut referenced_shm = HashSet::new();
    let mut pte = 0;
    let mut fds = 0;

    for process_info in &processes_info {
        pfns.par_extend(&process_info.pfns);
        anon_pfns.par_extend(&process_info.anon_pfns);
        swap_pages.par_extend(&process_info.swap_pages);
        anon_swap_pages.par_extend(&process_info.anon_swap_pages);
        referenced_shm.extend(&process_info.referenced_shms);
        // TODO: we can't sum PTE, this a theorical max value
        pte += process_info.pte;
        fds += process_info.fds;
    }

    ProcessGroupInfo {
        name: name.to_string(),
        processes_info,
        pfns,
        anon_pfns,
        swap_pages,
        anon_swap_pages,
        referenced_shm,
        pte,
        fds,
    }
}

/// Spawn new process with database user
/// return smon info
#[cfg(unix)]
pub fn get_smon_info(
    pid: i32,
    uid: u32,
    sid: &OsStr,
    home: &OsStr,
) -> Result<SmonInfo, Box<dyn std::error::Error>> {
    let myself = std::env::current_exe()?;

    let user = uzers::get_user_by_uid(uid).expect("Can't find user for uid {uid}");
    let gid = user.primary_group_id();

    let mut lib = home.to_os_string();
    lib.push("/lib");

    let mut cmd = Command::new(myself);
    cmd.env("LD_LIBRARY_PATH", lib)
        .env("ORACLE_SID", sid)
        .env("ORACLE_HOME", home)
        .uid(uid)
        .gid(gid)
        .arg("get-db-info")
        .args(["--pid", &format!("{pid}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(groups) = user.groups() {
        let groups: Vec<u32> = groups.iter().map(|g| g.gid()).collect();
        cmd.groups(&groups);
    }
    let child = cmd.spawn()?;
    let output = child.wait_with_output()?;

    if !output.status.success() {
        return Err(format!(
            "Proces failed for DB {sid:?} {uid} {home:?}: {:?}",
            output
        ))?;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    let smon_info: SmonInfo = serde_json::from_str(&stdout)?;
    Ok(smon_info)
}

#![allow(non_snake_case)]

// https://biriukov.dev/docs/page-cache/4-page-cache-eviction-and-page-reclaim/
// cat /proc/$(pidof cat)/smaps_rollup
// cat /proc/$(pidof cat)/status
// pmap -X $(pidof cat)
// smem --processfilter=cat
// pahole -C task_struct /sys/kernel/btf/vmlinux

use itertools::Itertools;
use procfs::{
    process::{MMapPath, Pfn, Process},
    PhysicalMemoryMap, PhysicalPageFlags,
};
use std::collections::{HashMap, HashSet};

use log::{info, warn};
use procfs::{
    process::{MemoryMap, PageInfo},
    Shm,
};

use oracle::{Connector, Privilege};
use std::ffi::OsString;

/// Convert pfn to index into non-contiguous memory mappings
pub fn pfn_to_index(iomem: &[PhysicalMemoryMap], page_size: u64, pfn: Pfn) -> Option<u64> {
    if pfn.0 == 0 {
        return None;
    }

    let mut previous_maps_size = 0;
    for map in iomem {
        let (pfn_start, pfn_end) = (map.address.0 / page_size, map.address.1 / page_size);
        if pfn.0 <= pfn_end {
            return Some(previous_maps_size + pfn.0 - pfn_start);
        }
        previous_maps_size += pfn_end - pfn_start;
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
pub fn get_pfn_count(iomem: &[PhysicalMemoryMap], page_size: u64) -> u64 {
    iomem
        .iter()
        .map(|map| map.address.1 / page_size - map.address.0 / page_size)
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

pub fn compute_compound_pages(
    data: &[(Pfn, u64, PhysicalPageFlags)],
) -> [u64; FLAG_NAMES.len() + 1] {
    let mut counters = [0u64; FLAG_NAMES.len() + 1];

    #[allow(unused_variables)]
    let mut merged_compound_pages = 0;
    let mut iter = data.iter().peekable();
    while let Some(&(_pfn, _count, flags)) = iter.next() {
        if flags.contains(PhysicalPageFlags::COMPOUND_HEAD) {
            let head_flags = flags;
            //println!("0: {:?}", head_flags);

            for (index, _) in FLAG_NAMES.iter().enumerate() {
                if head_flags.bits() & (1 << index) == 1 << index {
                    counters[index] += 1;
                }
            }

            for (_i, &(_pfn, _count, flags)) in iter
                .take_while_ref(|(_pfn, _count, flags)| {
                    flags.contains(PhysicalPageFlags::COMPOUND_TAIL)
                })
                .enumerate()
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

pub fn print_counters(counters: [u64; FLAG_NAMES.len() + 1]) {
    for (name, value) in FLAG_NAMES.iter().zip(counters) {
        let size = humansize::format_size(
            value * procfs::page_size(),
            humansize::BINARY.fixed_at(Some(humansize::FixedAt::Kilo)),
        );
        println!("{:15}: {}", name, size);
    }

    //let total_size = data.len() as u64 * procfs::page_size();
    //let total_size = humansize::format_size(
    //    total_size,
    //    humansize::BINARY.fixed_at(Some(humansize::FixedAt::Kilo)),
    //);
    //println!("{:15}: {}", "Total", total_size);
}

/// Scan each page of shm
/// Return None if shm uses any swap
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

            // try to read the memory
            // dump because this will load the whole mapping into RAM?
            let ptr = ptr as *mut u8;
            let mut dummy = 0;

            // only read if shm is not in swap
            if must_read {
                // we must read each page to populate pagemap
                let slice = std::slice::from_raw_parts_mut(ptr, shm.size as usize);
                for val in slice {
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

/// Connect to DB using OS auth and env vars
/// return size of SGA
pub fn get_db_info() -> Result<(u64, u64, u64, String), Box<dyn std::error::Error>> {
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
    let large_pages = conn.query_row_as::<String>(sql, &[])?;

    Ok((sga_size, processes, pga, large_pages))
}

/// Find smons processes
/// For each, return (pid, uid, ORACLE_SID, ORACLE_HOME)
pub fn find_smons() -> Vec<(i32, u32, OsString, OsString)> {
    let smons: Vec<Process> = procfs::process::all_processes()
        .unwrap()
        .filter_map(|proc| {
            let cmdline = proc.as_ref().ok()?.cmdline().ok()?;

            if cmdline.len() == 1
                && (cmdline[0].starts_with("ora_smon_") || cmdline[0].starts_with("asm_smon_"))
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

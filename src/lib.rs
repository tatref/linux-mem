// https://biriukov.dev/docs/page-cache/4-page-cache-eviction-and-page-reclaim/
// cat /proc/$(pidof cat)/smaps_rollup
// cat /proc/$(pidof cat)/status
// pmap -X $(pidof cat)
// smem --processfilter=cat
// pahole -C task_struct /sys/kernel/btf/vmlinux

use itertools::Itertools;
use procfs::{process::Pfn, PhysicalMemoryMap, PhysicalPageFlags};
use std::collections::{HashMap, HashSet};

use procfs::{
    process::{MemoryMap, PageInfo},
    Shm,
};

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
    data: &Vec<(Pfn, u64, PhysicalPageFlags)>,
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

pub fn shm2pfns(shm: &Shm) -> Result<HashSet<Pfn>, Box<dyn std::error::Error>> {
    let ptr: *mut libc::c_void;
    // TODO: fix procfs type
    let shmid: libc::c_int = shm.shmid as i32;
    let size = shm.size;

    // Map shared memory to current process
    {
        let shmaddr: *const libc::c_void = core::ptr::null();
        // we don't want any permission
        let shmflags: libc::c_int = libc::SHM_RDONLY;

        unsafe {
            ptr = libc::shmat(shmid, shmaddr, shmflags);
            if ptr == -1i32 as *mut libc::c_void {
                println!("shmat failed for shmid {shmid}");
                return Err(std::io::Error::last_os_error().into());
            }

            // try to read the memory
            let ptr = ptr as *mut u8;
            let mut dummy = 0;

            // we must read each page to create a mapping
            let slice = std::slice::from_raw_parts_mut(ptr, size as usize);
            for val in slice {
                dummy += *val;
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
        .filter(|map| map.address.0 == ptr as u64)
        .next()
        .ok_or("Map not found")?; // return if shared memory is not found

    let (start, end) = (
        map.address.0 / procfs::page_size(),
        map.address.1 / procfs::page_size(),
    );

    let mut pfns = HashSet::new();
    for page_info in pagemap.get_range_info((start as usize)..(end as usize))? {
        match page_info {
            PageInfo::MemoryPage(mem_page) => {
                let pfn = mem_page.get_page_frame_number();
                pfns.insert(pfn);
            }
            PageInfo::SwapPage(_swap_page) => (),
        }
    }

    Ok(pfns)
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

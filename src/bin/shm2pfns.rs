#![allow(non_snake_case)]

// Attach current process to shared memory segments from /proc/sysvipc/shm
// root is required

use std::collections::{HashMap, HashSet};

use procfs::process::Pfn;
use procfs::PhysicalPageFlags;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // (key, id) -> PFNs
    let mut h: HashMap<(i32, u64), HashSet<Pfn>> = HashMap::new();

    for shm in procfs::Shm::new().expect("Can't read /dev/sysvipc/shm") {
        let (pfns, _swap_pages, _pages_4k, _pages_2M) =
            snap::shm2pfns(&all_physical_pages, &shm, true)
                .expect("Got an error")
                .unwrap(); // we can unwrap because we force reads

        h.insert((shm.key, shm.shmid), pfns);
    }

    for (&(k, id), v) in h.iter() {
        println!("key: {k}, id: {id}: {} PFNs", v.len());
    }

    Ok(())
}

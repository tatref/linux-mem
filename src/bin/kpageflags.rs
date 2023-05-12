// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

use procfs::prelude::*;
use procfs::PhysicalMemoryMap;

fn main() {
    let iomem: Vec<PhysicalMemoryMap> = procfs::iomem()
        .unwrap()
        .iter()
        .filter_map(|(ident, map)| if *ident == 0 { Some(map.clone()) } else { None })
        .filter(|map| map.name == "System RAM")
        .collect();

    let mut kpageflags = procfs::KPageFlags::new().unwrap();

    for segment in iomem.iter() {
        eprintln!(
            "{} {:x}-{:x}: {} MiB",
            &segment.name,
            segment.address.0,
            segment.address.1,
            (segment.address.1 - segment.address.0) / 1024 / 1024
        );

        let (start_pfn, end_pfn) = segment.get_range().get();
        let flags = kpageflags.get_range_info(start_pfn, end_pfn).unwrap();

        for (idx, flag) in flags.iter().enumerate() {
            println!("0x{:x} {:?}", start_pfn.0 + idx as u64, flag);
        }
    }
}

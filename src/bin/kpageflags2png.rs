// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

use image::{ImageBuffer, Rgb, RgbImage};
use procfs::prelude::*;
use procfs::process::Pfn;
use procfs::{PhysicalMemoryMap, PhysicalPageFlags};

fn main() {
    let page_size = procfs::page_size();

    let iomem: Vec<PhysicalMemoryMap> = procfs::iomem()
        .unwrap()
        .iter()
        .filter_map(|(ident, map)| if *ident == 0 { Some(map.clone()) } else { None })
        .filter(|map| map.name == "System RAM")
        .collect();

    let mut kpageflags = procfs::KPageFlags::new().unwrap();

    let pfns = snap::get_pfn_count(&iomem, page_size);
    let order = (pfns as f64).log2() / 2.;
    let order = order.ceil() as u8;

    let segments: Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)> = iomem
        .iter()
        .map(|segment| {
            let (start_pfn, end_pfn) = segment.get_range().get();
            let flags: Vec<PhysicalPageFlags> =
                kpageflags.get_range_info(start_pfn, end_pfn).unwrap();

            (start_pfn, end_pfn, flags)
        })
        .collect();

    let mut default_img: RgbImage =
        ImageBuffer::new(2u32.pow(order as u32), 2u32.pow(order as u32));
    default_img.fill(0);

    for map in iomem.iter().filter(|map| map.name == "System RAM") {
        let (start, end) = map.get_range().get();
        for pfn in start.0..end.0 {
            let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();
            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

            if let Some(_idx) = snap::pfn_is_in_ram(&iomem, page_size, Pfn(pfn)) {
                default_img.put_pixel(x as u32, y as u32, Rgb([128, 128, 128]));
            }
        }
    }

    for current_flag in PhysicalPageFlags::all().iter_names() {
        println!("{}", current_flag.0.to_lowercase());
        let mut img = default_img.clone();

        for (start_pfn, end_pfn, flags) in segments.iter() {
            assert_eq!(end_pfn.0 - start_pfn.0, flags.len() as u64);

            for (pfn, &flag) in (start_pfn.0..end_pfn.0).zip(flags.iter()) {
                let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();
                let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

                let color_on = Rgb([0, 200, 200]);
                let color_off = Rgb([0, 100, 100]);

                if flag.contains(current_flag.1) {
                    img.put_pixel(x as u32, y as u32, color_on);
                } else {
                    img.put_pixel(x as u32, y as u32, color_off);
                }
            }
        }

        img.save(&format!("kpageflags_{}.png", current_flag.0.to_lowercase()))
            .unwrap();
    }
}

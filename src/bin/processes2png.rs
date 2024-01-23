// Tool to visualize memory fragmentation
// For details, see https://tatref.github.io/blog/2023-visual-linux-memory-compact/
//

use std::collections::HashSet;

use image::{ImageBuffer, Rgb, RgbImage};
use procfs::prelude::*;
use procfs::process::{MMapPath, Pfn, Process};

fn handle_process(process: &Process) -> Result<HashSet<Pfn>, Box<dyn std::error::Error>> {
    let mut pfn_set = HashSet::new();

    let page_size = procfs::page_size();

    let mut pagemap = process.pagemap()?;
    let memmap = process.maps()?;

    for memory_map in memmap {
        let mem_start = memory_map.address.0;
        let mem_end = memory_map.address.1;

        let page_start = (mem_start / page_size) as usize;
        let page_end = (mem_end / page_size) as usize;

        // can't scan Vsyscall, so skip it
        if memory_map.pathname == MMapPath::Vsyscall {
            continue;
        }

        for page_info in pagemap.get_range_info(page_start..page_end)? {
            match page_info {
                procfs::process::PageInfo::MemoryPage(memory_page) => {
                    let pfn = memory_page.get_page_frame_number();
                    pfn_set.insert(pfn);
                }
                procfs::process::PageInfo::SwapPage(_) => (),
            }
        }
    }

    Ok(pfn_set)
}

fn main() {
    let page_size = procfs::page_size();
    let iomem: Vec<_> = procfs::iomem()
        .expect("Can't read /proc/iomem")
        .iter()
        .map(|(_indent, map)| map.clone())
        .filter(|map| map.name == "System RAM")
        .collect();

    let processes: Vec<Process> = procfs::process::all_processes()
        .unwrap()
        .filter_map(|res| res.ok())
        .collect();

    let mut pfn_set = HashSet::new();
    for p in processes.iter() {
        let some_pfns = match handle_process(p) {
            Ok(x) => x,
            Err(e) => {
                println!("{:?}", e);
                continue;
            }
        };
        pfn_set = pfn_set.union(&some_pfns).copied().collect();

        //dbg!(p.pagemap());
    }

    // draw

    let pfns = snap::get_pfn_count(&iomem, page_size);
    dbg!(pfns);
    let order = (pfns as f64).log2() / 2.;
    dbg!(order);
    let order = order.ceil() as u8;
    dbg!(order);

    let mut img: RgbImage = ImageBuffer::new(2u32.pow(order as u32), 2u32.pow(order as u32));
    let _gradient = colorgrad::rainbow();
    dbg!(img.dimensions());

    img.fill(0);

    let colors = [Rgb([255, 0, 0]), Rgb([0, 255, 0]), Rgb([0, 0, 255])];

    for map in iomem.iter().filter(|map| map.name == "System RAM") {
        let (start, end) = map.get_range().get();
        for pfn in start.0..end.0 {
            let index = snap::pfn_to_index(&iomem, Pfn(pfn)).unwrap();
            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

            if let Some(_idx) = snap::pfn_is_in_ram(&iomem, page_size, Pfn(pfn)) {
                let c = colors[0].0;
                let div = 4;
                let pixel = Rgb([c[0] / div, c[1] / div, c[2] / div]);
                img.put_pixel(x as u32, y as u32, pixel);
            }
        }
    }

    for &pfn in &pfn_set {
        if pfn.0 == 0 {
            continue;
        }
        let index = snap::pfn_to_index(&iomem, pfn).unwrap();
        let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

        let pixel;
        if let Some(_idx) = snap::pfn_is_in_ram(&iomem, page_size, pfn) {
            pixel = colors[0];
        } else {
            // out of ram
            unreachable!();
        }
        img.put_pixel(x as u32, y as u32, pixel);
    }

    img.save("img.png").unwrap();
}

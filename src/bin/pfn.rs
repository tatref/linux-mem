use std::fs::File;
use std::os::unix::prelude::FileExt;
use std::path::PathBuf;

use colorgrad::Gradient;
use image::ImageBuffer;
use image::Rgb;
use image::RgbImage;
use procfs::process::MMapPath;
use procfs::process::Process;

fn handle_process(process: Process) {
    let page_size = procfs::page_size().unwrap();

    let mut pagemap = process.pagemap().unwrap();
    let memmap = process.maps().unwrap();

    for memory_map in memmap {
        let mem_start = memory_map.address.0;
        let mem_end = memory_map.address.1;

        let index_start = (mem_start / page_size) as usize;
        let index_end = (mem_end / page_size) as usize;

        // can't scan Vsyscall, so skip it
        match &memory_map.pathname {
            MMapPath::Vsyscall => continue,
            _ => (),
        }

        println!("0x{:x} {:?}", mem_start, memory_map);

        for index in index_start..index_end {
            let virt_mem = index * page_size as usize;
            let page_info = pagemap.get_info(index).unwrap();
            match page_info {
                procfs::process::PageInfo::MemoryPage(memory_page) => {
                    let pfn = memory_page.get_page_frame_number();
                    let phys_addr = pfn * page_size;
                    println!(
                        "virt_mem: 0x{:x}, pfn: 0x{:x}, phys_addr: 0x{:x}",
                        virt_mem, pfn, phys_addr
                    );
                }
                procfs::process::PageInfo::SwapPage(_) => todo!(),
            }
        }
    }
}

fn main() {
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        panic!("Run me as root");
    }

    let processes = procfs::process::all_processes()
        .unwrap()
        .filter(|p| {
            let Ok(p) = p.as_ref() else {
	return false;
};

            let Ok(exe) = p.exe() else {
			return false;
		};

            if exe == PathBuf::from("/usr/bin/cat") {
                return true;
            } else {
                return false;
            }
        })
        .map(|p| p.unwrap());

    for process in processes {
        println!("### {}", process.pid);
        handle_process(process);
    }

    let page_size = procfs::page_size().unwrap();
    let mem_start = 0x100000000u64;
    let mem_start = 0x000000000u64;
    let mem_end = 0x21fffffffu64;
    let pfn_start = mem_start / page_size;
    let pfn_end = mem_end / page_size;

    let pfns = pfn_end - pfn_start;
    let order = (pfns as f64).log2() / 2.;
    let order = order.ceil() as u8;

    let mut img: RgbImage = ImageBuffer::new(2u32.pow(order as u32), 2u32.pow(order as u32));
    let gradient = colorgrad::rainbow();
    dbg!(img.dimensions());

    let kpagecount = File::open("/proc/kpagecount").unwrap();
    let mut buf = [0; 8];

    let mut counter = 0;
    let mut ok = 0;
    let mut err = 0;

    let mut max = 0;
    for pfn in pfn_start..pfn_end {
        let entry_size = 64 / 8; // 64 bits = 8 bytes
        let offset = pfn * entry_size;

        match kpagecount.read_exact_at(&mut buf, offset) {
            Ok(_) => ok += 1,
            Err(_) => err += 1,
        }

        let page_references: u64 = u64::from_le_bytes(buf);
        if page_references != 0 {
            //println!("0x{:x}: {}", pfn, page_references);
        }
        counter += 1;

        max = page_references.max(max);

        let (x, y) = fast_hilbert::h2xy::<u64>(pfn.into(), order);
        let pixel = color_map(page_references, &gradient);
        img.put_pixel(x as u32, y as u32, pixel);
        //dbg!(x, y);
    }
    dbg!(counter);
    dbg!(err);
    dbg!(ok);
    dbg!(max);

    img.save("img.png").unwrap();
}

fn color_map(value: u64, gradient: &Gradient) -> Rgb<u8> {
    let value = value as u8;
    let MAX = 150.;

    match value {
        0 => Rgb([255, 255, 255]),
        v => {
            let c = gradient.at(v as f64 / MAX).to_rgba8();
            Rgb([c[0], c[1], c[2]])
        }
    }
}

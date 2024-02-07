// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

// https://crates.io/crates/tcp_message_io

use std::collections::HashSet;
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use procfs::process::{MMapPath, Pfn, Process};
use procfs::{prelude::*, KPageFlags};
use procfs::{PhysicalMemoryMap, PhysicalPageFlags};
use snap::aaa::*;

fn get_process_pfns(process: &Process) -> Result<HashSet<Pfn>, Box<dyn std::error::Error>> {
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

fn get_process_info(process: &Process) -> Result<ProcessInfo, Box<dyn std::error::Error>> {
    let pfns = get_process_pfns(process)?;
    let exe = process.exe()?;
    let pid = process.pid;

    Ok(ProcessInfo { pid, exe, pfns })
}

fn get_all_processes_info() -> Vec<ProcessInfo> {
    use rayon::prelude::*;
    let all_processes: Vec<Process> = procfs::process::all_processes()
        .unwrap()
        .filter_map(|p| p.ok())
        .collect();
    let processes_info: Vec<ProcessInfo> = all_processes
        .par_iter()
        .filter_map(|p| get_process_info(&p).ok())
        .collect();

    processes_info
}

fn get_segments(
    iomem: &[PhysicalMemoryMap],
    kpageflags: &mut KPageFlags,
) -> Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)> {
    let segments: Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)> = iomem
        .iter()
        .map(|segment| {
            let (start_pfn, end_pfn) = segment.get_range().get();
            let flags: Vec<PhysicalPageFlags> =
                kpageflags.get_range_info(start_pfn, end_pfn).unwrap();

            (start_pfn, end_pfn, flags)
        })
        .collect();

    segments
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args.get(1).unwrap_or(&"10000".to_string()).parse().unwrap();

    println!("Listening on :{:?}", port);
    let listener = TcpListener::bind(&format!("127.0.0.1:{}", port))
        .expect(&format!("Can't bind to port {}", port));

    let (mut socket, _client_addr) = listener.accept().expect("Can't get client");
    //socket.read_to_end(&mut buf).unwrap();
    //dbg!(&buf);

    let iomem: Vec<PhysicalMemoryMap> = procfs::iomem()
        .unwrap()
        .iter()
        .filter_map(|(ident, map)| if *ident == 0 { Some(map.clone()) } else { None })
        .filter(|map| map.name == "System RAM")
        .collect();

    let mut kpageflags = procfs::KPageFlags::new().unwrap();

    let mut processes_info = get_all_processes_info();
    let mut memory_segments = get_segments(&iomem, &mut kpageflags);
    let page_size = procfs::page_size();

    let message = Message::FirstUpdate(FirstUpdateMessage {
        page_size,
        processes_info,
        memory_segments,
        iomem: iomem.clone(),
    });
    message.send(&mut socket).unwrap();

    loop {
        let chrono = Instant::now();
        processes_info = get_all_processes_info();
        memory_segments = get_segments(&iomem, &mut kpageflags);
        let collect_duration = chrono.elapsed();
        dbg!(collect_duration);

        let message = Message::Update(UpdateMessage {
            processes_info,
            memory_segments,
            iomem: iomem.clone(),
        });

        let message_size = message.send(&mut socket).unwrap();
        eprintln!("message_size: {} MiB", message_size / 1024 / 1024);

        let update_duration = chrono.elapsed();
        dbg!(update_duration);

        thread::sleep(Duration::from_millis(2_000));
    }
}

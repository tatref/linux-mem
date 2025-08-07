// A tool to mmap a file multiple times into memory
// Usage:
//   dd if=/dev/zero of=bigfile.dd bs=1M count=1024
//   mmap bigfile.dd 10
//
// Will map "bigfile.dd" 10 times into memory. Memory usage should be ~1GiB, but most tools will report 10 GiB
//
//    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND
//   5278 tatref    20   0   10.0g  10.0g  10.0g S   0.0 137.0   0:00.51 mmap
//
// memstats will report correct value
// # memstats groups -c 'comm(mmap)'
// ...
//  Custom splitter
//  ┌────────────┬───────┬────────────┬────────────┬────────────┬───────────┬──────────┬──────────┬─────────┬──────────┐
//  │ group_name │ procs │ mem_rss    │ mem_anon   │ mem_uss    │ swap_anon │ swap_rss │ swap_uss │ shm_mem │ shm_swap │
//  ├────────────┼───────┼────────────┼────────────┼────────────┼───────────┼──────────┼──────────┼─────────┼──────────┤
//  │ Other      │ 103   │ 2250.68 MB │ 1874.86 MB │ 2249.11 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 0 MB    │ 0 MB     │
//  │ comm(mmap) │ 1     │ 1075.79 MB │ 0.07 MB    │ 1074.22 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 0 MB    │ 0 MB     │
//  └────────────┴───────┴────────────┴────────────┴────────────┴───────────┴──────────┴──────────┴─────────┴──────────┘

use std::fs::File;
use std::io::Write;

use memmap2::Mmap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file_name = std::env::args().nth(1).expect("filename");
    let count: usize = std::env::args()
        .nth(2)
        .expect("count")
        .parse()
        .expect("count must be a number");

    let file = File::open(&file_name)?;

    println!("mmapping...");

    let v: Vec<_> = (0..count)
        .filter_map(|x| {
            let mmap = unsafe { Mmap::map(&file).ok()? };
            print!("{x} ");
            std::io::stdout().lock().flush().unwrap();

            let mut dummy = 0;
            for x in &*mmap {
                // for read memory
                dummy ^= x;
            }
            // prevent compiler optimization
            std::hint::black_box(dummy);
            Some(mmap)
        })
        .collect();

    println!("waiting...");

    std::thread::sleep(std::time::Duration::from_secs(3600));
    Ok(())
}

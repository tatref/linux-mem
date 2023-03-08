// Attach current process to shared memory segments from /proc/sysvipc/shm
// root is required

use std::collections::{HashMap, HashSet};

use procfs::process::Pfn;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // (key, id) -> PFNs
    let mut h: HashMap<(i32, u64), HashSet<Pfn>> = HashMap::new();

    for shm in procfs::Shm::new().expect("Can't read /dev/sysvipc/shm") {
        let (pfns, _swap_pages) = snap::shm2pfns(&shm, true).expect("Got an error");

        h.insert((shm.key, shm.shmid), pfns);
    }

    for (&(k, id), v) in h.iter() {
        println!("key: {k}, id: {id}: {} PFNs", v.len());
    }

    Ok(())
}

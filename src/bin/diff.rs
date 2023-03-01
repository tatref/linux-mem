#![allow(dead_code)]
#![allow(unused_imports)]

use flate2::read::GzDecoder;
use procfs::process::{MMapPath, Process};
use procfs::Shm;
use std::collections::HashSet;
use std::fs::File;
use std::path::PathBuf;
use std::{collections::HashMap, ffi::OsString, path::Path, string::ParseError};
use tar::Archive;

struct Snapshot {
    processes: Vec<Process>,
}

impl Snapshot {
    fn load<P: AsRef<Path>>(path: P) -> Result<Self, ()> {
        fn untar(path: &Path) -> Result<(), ()> {
            let file = File::open(&path).unwrap();
            let tar = GzDecoder::new(file);
            let mut archive = Archive::new(tar);
            archive.unpack(".").map_err(|_| ())?;

            Ok(())
        }

        let mut snap_dir: PathBuf = if path
            .as_ref()
            .components()
            .last()
            .map(|p| p.as_os_str().to_string_lossy().to_string())
            .unwrap()
            .ends_with(".tar.gz")
        {
            let name = path.as_ref().to_string_lossy().to_string();
            let name = name.strip_suffix(".tar.gz").unwrap();
            PathBuf::from(name)
        } else {
            path.as_ref().to_owned()
        };

        if !snap_dir.exists() {
            untar(path.as_ref())?;
        }

        snap_dir.push("proc");
        dbg!(&snap_dir);

        let processes: Vec<_> = procfs::process::all_processes_with_root(snap_dir)
            .unwrap()
            .map(|p| p.unwrap())
            .collect();

        Ok(Self { processes })
    }
}

fn main() {
    /*
    let snap1 = std::env::args().nth(1).expect("Enter path to snapshot");
    let snap2 = std::env::args().nth(2).expect("Enter path to snapshot");

    let chrono = std::time::Instant::now();

    let snap1 = Snapshot::load(snap1).unwrap();
    let snap2 = Snapshot::load(snap2).unwrap();

    let elapsed = chrono.elapsed();
    println!("Loaded snapshots in {:?}", elapsed);

    dbg!(snap1.processes.len());

    let p1 = snap1
        .processes
        .iter()
        .filter(|p| {
            p.cmdline().expect("can't read cmdline").first()
                == Some(&"/usr/lib64/firefox/firefox".to_string())
        })
        .next()
        .unwrap();

    dbg!(p1);
    dbg!(p1.cmdline(), p1.exe());

    let pagemap = p1.pagemap().unwrap();

    return;
    let p2 = snap2
        .processes
        .iter()
        .filter(|p| p.pid == 9892)
        .next()
        .unwrap();

    let smaps1 = p1.smaps().unwrap();
    let smaps2 = p2.smaps().unwrap();

    let smaps1: HashMap<_, _> = smaps1
        .iter()
        .map(|x| (x.0.address.0, (&x.0, &x.1)))
        .collect();
    let smaps2: HashMap<_, _> = smaps2
        .iter()
        .map(|x| (x.0.address.0, (&x.0, &x.1)))
        .collect();

    let k1 = smaps1.keys().collect::<HashSet<_>>();
    let k2 = smaps2.keys().collect::<HashSet<_>>();

    let removed = k1.difference(&k2);
    let added = k2.difference(&k1);
    let same = k2.intersection(&k1);

    println!("Removed:");
    for addr in removed {
        dbg!(smaps1.get(addr));
    }

    println!("\nAdded:");
    for addr in added {
        dbg!(smaps2.get(addr));
    }

    println!("\nDifferences:");
    for k in same {
        let m1 = smaps1.get(k).unwrap();
        let m2 = smaps2.get(k).unwrap();
        if m1.1.map != m2.1.map {
            println!("0x{:X}", m1.0.address.0);

            let stats = m1.1.map.keys();

            for stat in stats {
                let stat_1 = m1.1.map.get(stat).unwrap();
                let stat_2 = m2.1.map.get(stat).unwrap();

                if stat_1 != stat_2 {
                    println!("{}: {} -> {}", stat, stat_1, stat_2);
                }
            }
        }
    }

    println!("\nDone");
    */
}

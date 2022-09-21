#![allow(dead_code)]
#![allow(unused_imports)]

use flate2::read::GzDecoder;
use procfs::process::MMapPath;
use procfs::Shm;
use std::fs::File;
use std::path::PathBuf;
use std::{collections::HashMap, ffi::OsString, path::Path, string::ParseError};
use tar::Archive;

struct Snapshot {}

impl Snapshot {
    fn load<P: AsRef<Path>>(mut path: P) -> Result<Self, ()> {
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
            untar(path.as_ref());
        }

        snap_dir.push("proc");
        dbg!(&snap_dir);

        for p in procfs::process::all_processes_with_root(snap_dir).unwrap() {
            let p = p.unwrap();
            println!("{:?}", p);
            println!("{:?}", p.exe());

            break;
        }

        Err(())
    }
}

fn main() {
    let snap1 = std::env::args().nth(1).expect("Enter path to snapshot");
    //let snap2 = std::env::args().nth(2).expect("Enter path to snapshot");

    let chrono = std::time::Instant::now();

    let snap1 = Snapshot::load(snap1).unwrap();
    //let snap2 = Snapshot::load(snap2).unwrap();

    let elapsed = chrono.elapsed();
    dbg!(elapsed);
}

// Tool to play with Linux shared memory
// C examples here: https://github.com/torvalds/linux/blob/master/tools/testing/selftests/vm/hugepage-shm.c
//

use std::mem::MaybeUninit;

use clap::{arg, value_parser, Command};
use libc::{c_void, shmid_ds};

fn wait() {
    println!("ENTER to exit");

    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf).unwrap();
}

fn create_shm(key: i32, size: usize, huge_page: bool) {
    // read/write for everyone
    let perms = 0o666;

    // IPC_CREAT: create segment
    // IPC_EXCL: error if key already exists
    let mut shmflg = libc::IPC_CREAT | libc::IPC_EXCL | perms;

    if huge_page {
        shmflg |= libc::SHM_HUGETLB;
    }

    let shmid = unsafe { libc::shmget(key, size, shmflg) };
    if shmid < 0 {
        let errno = unsafe { *libc::__errno_location() };
        panic!("ERROR: shmget failed: {:?}", errno);
    }
    println!("INFO: got shmid {}", shmid);
}

fn read_shm(shmid: i32, size: usize, _wait: bool) {
    let flags = libc::SHM_RDONLY;

    let ptr = unsafe { libc::shmat(shmid, std::ptr::null::<c_void>(), flags) as *mut u8 };
    if ptr == (-1isize) as *mut u8 {
        let errno = unsafe { *libc::__errno_location() };

        panic!("ERROR: shmat failed: {}", errno);
    }

    println!("INFO: got ptr: {:p}", ptr);

    let mut total = 0;
    for i in 0..size {
        unsafe {
            let ptr2 = ptr.add(i);

            let val: u8 = ptr2.read();
            total += val;
        }
    }
    dbg!(total);

    if _wait {
        wait();
    }
}

fn write_shm(shmid: i32, size: usize) {
    let flags = libc::SHM_W;

    let ptr = unsafe { libc::shmat(shmid, std::ptr::null::<c_void>(), flags) as *mut u8 };
    if ptr == (-1isize) as *mut u8 {
        let errno = unsafe { *libc::__errno_location() };

        panic!("ERROR: shmat failed: {}", errno);
    }

    println!("INFO: got ptr: {:p}", ptr);

    for i in 0..size {
        unsafe {
            let ptr2 = ptr.add(i);

            let val: u8 = rand::random();
            ptr2.write(val);
        }
    }
}

fn delete_shm(shmid: i32) {
    let cmd = libc::IPC_RMID;
    let mut buf: MaybeUninit<shmid_ds> = MaybeUninit::uninit();

    let ret = unsafe { libc::shmctl(shmid, cmd, buf.as_mut_ptr()) };
    if ret != 0 {
        let errno = unsafe { *libc::__errno_location() };
        panic!("ERROR: RMID failed: {}", errno);
    }
}

fn info() {
    for shm in procfs::Shm::new().expect("Can't read sysvipc") {
        println!("{:?}", shm);
    }
}

fn main() {
    let matches = Command::new("MyApp")
        .version("1.0")
        .author("Tatref https://github.com/tatref")
        .about("Playing with shared memory")
        .subcommand_required(true)
        .subcommand(Command::new("info"))
        .subcommand(
            Command::new("create").args([
                arg!(<KEY>)
                    .value_parser(value_parser!(i32))
                    .help("Key from \"info\" command"),
                arg!(<SIZE>)
                    .value_parser(value_parser!(usize))
                    .help("Size in bytes"),
                arg!(<HUGE_PAGES>).value_parser(value_parser!(bool)).help("Use large pages. You must allocates larges pages -- see /proc/sys/vm/nr_hugepages"),
            ])
        )
        .subcommand(
            Command::new("write").args([
                arg!(<SHMID>)
                    .value_parser(value_parser!(i32))
                    .help("Shmid from \"info\" command"),
                arg!(<SIZE>)
                    .value_parser(value_parser!(usize))
                    .help("Size in bytes")
                ])
            )
        .subcommand(
            Command::new("read").args([
                arg!(<SHMID>)
                    .value_parser(value_parser!(i32))
                    .help("Shmid from \"info\" command"),
                arg!(<SIZE>)
                    .value_parser(value_parser!(usize))
                    .help("Size in bytes"),
                arg!(<WAIT>)
                    .value_parser(value_parser!(bool))
                    .help("Wait for user input after finished reading"),
                ])
            )
        .subcommand(Command::new("delete").arg(arg!(<SHMID>).value_parser(value_parser!(i32))
    .help("Shmid from \"info\" command")))
        .get_matches();

    match matches.subcommand() {
        Some(("info", _)) => info(),
        Some(("create", sub)) => {
            let key = *sub.get_one::<i32>("KEY").unwrap();
            let size = *sub.get_one::<usize>("SIZE").unwrap();
            let huge_pages = *sub.get_one::<bool>("HUGE_PAGES").unwrap();

            create_shm(key, size, huge_pages);
        }
        Some(("delete", sub)) => {
            let shmid = *sub.get_one::<i32>("SHMID").unwrap();

            delete_shm(shmid);
        }
        Some(("read", sub)) => {
            let shmid = *sub.get_one::<i32>("SHMID").unwrap();
            let size = *sub.get_one::<usize>("SIZE").unwrap();
            let wait = *sub.get_one::<bool>("WAIT").unwrap();

            read_shm(shmid, size, wait);
        }
        Some(("write", sub)) => {
            let shmid = *sub.get_one::<i32>("SHMID").unwrap();
            let size = *sub.get_one::<usize>("SIZE").unwrap();

            write_shm(shmid, size);
        }
        _ => unreachable!(),
    }
}

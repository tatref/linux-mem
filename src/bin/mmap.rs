use std::os::{linux::fs::MetadataExt, unix::prelude::AsRawFd};

use libc::FALLOC_FL_UNSHARE_RANGE;

fn wait() {
    println!("press enter");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
}

fn main() {
    let f = std::fs::File::open("bigfile.dd").unwrap();
    let len = f.metadata().unwrap().st_size();
    let fd = f.as_raw_fd();

    wait();

    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len as usize,
            libc::PROT_WRITE,
            libc::MAP_PRIVATE,
            fd,
            0,
        ) as *mut u8
    };

    println!("ptr: {:?}", ptr);

    if ptr.is_null() {
        println!("mmap failed");
        return;
    }

    wait();

    for x in 0..len {
        let val: u8 = rand::random();
        unsafe { ptr.offset(x as isize).write(val) };
    }

    wait();
}

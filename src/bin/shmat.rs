// Attach current process to shared memory segments from /proc/sysvipc/shm
// root is required

use procfs::Current;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for shm in procfs::SharedMemorySegments::current()
        .expect("Can't read /dev/sysvipc/shm")
        .0
    {
        let shmid: libc::c_int = shm.shmid as i32; // TODO: fix procfs type
        let key = shm.key;
        let size = shm.size;

        // a new ptr will be returned
        let shmaddr: *const libc::c_void = core::ptr::null();
        // we don't ant any permission
        let shmflags: libc::c_int = 0;

        unsafe {
            let ptr: *mut libc::c_void = libc::shmat(shmid, shmaddr, shmflags);
            if ptr == -1i32 as *mut libc::c_void {
                println!("shmat failed");
                dbg!(std::io::Error::last_os_error());
            }
            println!("key: {key:>10}, shmid: {shmid:>5}, size: {size:>12} B, ptr: {ptr:p}");
        }
    }

    let wait = 60;
    std::thread::sleep(std::time::Duration::from_secs(wait));

    Ok(())
}

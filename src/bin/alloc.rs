fn wait() {
    println!("press enter");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
}

fn main() {
    let x: u8 = rand::random();
    let mut data = vec![x; 100 * 1024 * 1024];

    wait();

    let pid = unsafe { libc::fork() };
    println!("forked");

    std::thread::sleep(std::time::Duration::from_secs_f32(10.));

    if pid == 0 {
        // child
        let x: u8 = rand::random();
        for ptr in data.iter_mut() {
            *ptr = x;
        }
        println!("Child mutated data");

        std::thread::sleep(std::time::Duration::from_secs_f32(10.));
        println!("Child exits");
    } else {
        // parent
        std::thread::sleep(std::time::Duration::from_secs_f32(10.));
        println!("Parent exits");
    }
}

// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

// https://crates.io/crates/tcp_message_io

use std::{
    net::TcpStream,
    thread,
    time::{Duration, Instant},
};

use snap::aaa::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let remote = args.get(1).unwrap();

    let mut socket = TcpStream::connect(remote).unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);

    let socket_thread = thread::spawn(move || loop {
        let message = Message::read(&mut socket).unwrap();
        tx.try_send(message).unwrap();
    });

    'mainloop: loop {
        let chrono = Instant::now();

        if let Ok(message) = rx.try_recv() {
            println!("got message");
            match message {
                Message::Update(message) => {
                    for p in message.processes_info.iter() {
                        //dbg!(p.pid, &p.exe);
                    }
                }
                Message::Finish => {
                    break 'mainloop;
                }
            }
        }

        let elapsed = chrono.elapsed();
        dbg!(elapsed);

        //println!("no message");
        thread::sleep(Duration::from_millis(1));
    }

    socket_thread.join().unwrap();
}

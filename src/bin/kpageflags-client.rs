// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

// https://crates.io/crates/tcp_message_io

use std::{
    net::TcpStream,
    thread,
    time::{Duration, Instant},
};

use macroquad::{color::Color, texture::Image};
use procfs::process::Pfn;
use snap::aaa::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let remote = args.get(1).unwrap();

    let mut socket = TcpStream::connect(remote).unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);

    let socket_thread = thread::spawn(move || loop {
        let message = Message::recv(&mut socket).unwrap();
        tx.try_send(message).unwrap();
    });

    let mut default_img: Option<Image> = None;
    let page_size = 4096;

    'mainloop: loop {
        let chrono = Instant::now();

        if let Ok(message) = rx.try_recv() {
            println!("got message");
            match message {
                Message::FirstUpdate(message) => {
                    let pfns = snap::get_pfn_count(&message.iomem);
                    let order = (pfns as f64).log2() / 2.;
                    let order = order.ceil() as u8;

                    default_img = Some(Image::gen_image_color(
                        2u16.pow(order as u32),
                        2u16.pow(order as u32),
                        Color::from_rgba(79, 79, 79, 255),
                    ));

                    //for map in message.iomem.iter() {
                    for (start, end, _) in &message.memory_segments {
                        //let (start, end) = map.get_range().get();
                        for pfn in start.0..end.0 {
                            let index =
                                snap::pfn_to_index(&message.iomem, page_size, Pfn(pfn)).unwrap();
                            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

                            default_img.as_mut().unwrap().set_pixel(
                                x as u32,
                                y as u32,
                                Color::from_rgba(64, 64, 64, 255),
                            );
                        }
                    }
                }
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

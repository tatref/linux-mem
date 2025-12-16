// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

use std::{net::SocketAddr, process::Command, thread, time::Duration};

use messages::*;

pub mod messages {
    use std::{
        collections::HashSet,
        io::{Read, Write},
        net::TcpStream,
        path::PathBuf,
    };

    use procfs_core::{process::Pfn, PhysicalMemoryMap, PhysicalPageFlags};
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    pub enum Message {
        FirstUpdate(FirstUpdateMessage),
        Update(UpdateMessage),
        Finish,
        //ServerParams(ServerParamsMessage),
    }

    impl Message {
        pub fn send(&self, socket: &mut TcpStream) -> Result<usize, Box<dyn std::error::Error>> {
            let buf = rmp_serde::to_vec(&self)?;

            //let buf = bincode::serialize(&self)?;
            //dbg!(buf.len());

            //let mut buf = Vec::new();
            //ciborium::into_writer(&self, &mut buf).unwrap();
            //dbg!(buf.len());

            let size = (buf.len() as u64).to_le_bytes();
            socket.write_all(&size)?;
            socket.write_all(&buf)?;

            Ok(buf.len())
        }

        pub fn recv(socket: &mut TcpStream) -> Result<Self, Box<dyn std::error::Error>> {
            let mut size = [0u8; 8];
            socket.read_exact(&mut size)?;
            let size = u64::from_le_bytes(size);
            dbg!(size);

            let max_message_size = std::env::vars()
                .filter_map(|(k, v)| {
                    if k == "MAX_MESSAGE_SIZE" {
                        let value: Option<u64> = v.parse().ok();
                        value
                    } else {
                        None
                    }
                })
                .next()
                .unwrap_or(20);

            if size > max_message_size * 1024 * 1024 {
                return Err(format!("Message is too big! ({} MiB)", size / 1024 / 1024).into());
            }
            let mut buf: Vec<u8> = vec![0u8; size as usize];
            socket.read_exact(&mut buf)?;

            let message: Message = match rmp_serde::from_slice(&buf) {
                Ok(m) => m,
                Err(e) => panic!("deserialization failed {:?}", e),
            };

            Ok(message)
        }
    }

    #[derive(Serialize, Deserialize)]
    pub struct FirstUpdateMessage {
        pub page_size: u64,
        pub update_message: UpdateMessage,
        //pub processes_info: Vec<ProcessInfo>,
        //pub memory_segments: Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)>,
        //pub iomem: Vec<PhysicalMemoryMap>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct UpdateMessage {
        pub processes_info: Vec<ProcessInfo>,
        pub memory_segments: Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)>,
        pub iomem: Vec<PhysicalMemoryMap>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct ProcessInfo {
        pub pid: i32,
        pub exe: PathBuf,
        pub pfns: HashSet<Pfn>,
    }
}

#[cfg(unix)]
pub mod server {
    use std::collections::HashSet;
    use std::net::{SocketAddr, TcpListener};
    use std::thread;
    use std::time::{Duration, Instant};

    //use procfs::prelude::*;
    use procfs::process::{MMapPath, Pfn, Process};
    use procfs::{KPageFlags, PhysicalMemoryMap, WithCurrentSystemInfo};
    use procfs_core::PhysicalPageFlags;

    use crate::{FirstUpdateMessage, Message, ProcessInfo, UpdateMessage};

    pub fn get_process_pfns(process: &Process) -> Result<HashSet<Pfn>, Box<dyn std::error::Error>> {
        let mut pfn_set = HashSet::new();

        let page_size = procfs::page_size();

        let mut pagemap = process.pagemap()?;
        let memmap = process.maps()?;

        for memory_map in memmap {
            let mem_start = memory_map.address.0;
            let mem_end = memory_map.address.1;

            let page_start = (mem_start / page_size) as usize;
            let page_end = (mem_end / page_size) as usize;

            // can't scan Vsyscall, so skip it
            if memory_map.pathname == MMapPath::Vsyscall {
                continue;
            }

            for page_info in pagemap.get_range_info(page_start..page_end)? {
                match page_info {
                    procfs::process::PageInfo::MemoryPage(memory_page) => {
                        let pfn = memory_page.get_page_frame_number();
                        pfn_set.insert(pfn);
                    }
                    procfs::process::PageInfo::SwapPage(_) => (),
                }
            }
        }

        Ok(pfn_set)
    }

    pub fn get_process_info(process: &Process) -> Result<ProcessInfo, Box<dyn std::error::Error>> {
        let pfns = get_process_pfns(process)?;
        let exe = process.exe()?;
        let pid = process.pid;

        Ok(ProcessInfo { pid, exe, pfns })
    }

    pub fn get_all_processes_info() -> Vec<ProcessInfo> {
        use rayon::prelude::*;
        let all_processes: Vec<Process> = procfs::process::all_processes()
            .unwrap()
            .filter_map(|p| p.ok())
            .collect();
        let processes_info: Vec<ProcessInfo> = all_processes
            .par_iter()
            .filter_map(|p| get_process_info(p).ok())
            .collect();

        processes_info
    }

    pub fn get_memory_zones_flags(
        iomem: &[PhysicalMemoryMap],
        kpageflags: &mut KPageFlags,
    ) -> Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)> {
        let segments: Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)> = iomem
            .iter()
            .map(|segment| {
                let (start_pfn, end_pfn) = segment.get_range().get();
                let flags: Vec<PhysicalPageFlags> =
                    kpageflags.get_range_info(start_pfn, end_pfn).unwrap();

                (start_pfn, end_pfn, flags)
            })
            .collect();

        segments
    }

    pub fn server(socket: SocketAddr) {
        let listener = TcpListener::bind(socket)
            .unwrap_or_else(&|_| panic!("Can't bind to socket {}", socket));
        println!("Listening on :{:?}", socket);

        let (mut socket, _client_addr) = listener.accept().expect("Can't get client");

        let iomem: Vec<PhysicalMemoryMap> = procfs::iomem()
            .unwrap()
            .iter()
            .filter_map(|(ident, map)| if *ident == 0 { Some(map.clone()) } else { None })
            .filter(|map| map.name == "System RAM")
            .collect();

        let mut kpageflags = procfs::KPageFlags::new().unwrap();

        let mut processes_info = get_all_processes_info();
        let mut memory_segments = get_memory_zones_flags(&iomem, &mut kpageflags);
        let page_size = procfs::page_size();

        let message = Message::FirstUpdate(FirstUpdateMessage {
            page_size,
            update_message: UpdateMessage {
                processes_info,
                memory_segments,
                iomem: iomem.clone(),
            },
        });
        message.send(&mut socket).unwrap();

        let target_update_interval = 2.;
        loop {
            let chrono = Instant::now();
            processes_info = get_all_processes_info();
            memory_segments = get_memory_zones_flags(&iomem, &mut kpageflags);

            let message = Message::Update(UpdateMessage {
                processes_info,
                memory_segments,
                iomem: iomem.clone(),
            });

            let message_size = message.send(&mut socket).unwrap();
            eprintln!("message_size: {} MiB", message_size / 1024 / 1024);

            let update_duration = chrono.elapsed();
            dbg!(update_duration);

            let sleep = (target_update_interval - update_duration.as_secs_f64()).max(0.);
            eprintln!("Sleeping for {:.2}", sleep);
            thread::sleep(Duration::from_secs_f64(sleep));
        }
    }
}

mod client {
    use std::{
        net::{SocketAddr, TcpStream},
        thread,
        time::Instant,
    };

    use itertools::Itertools;
    use macroquad::{
        color::*,
        input::{is_key_pressed, mouse_position, mouse_wheel, KeyCode},
        math::Vec2,
        miniquad::FilterMode,
        texture::{draw_texture_ex, DrawTextureParams, Image, Texture2D},
        window::{clear_background, next_frame},
    };
    use procfs_core::{process::Pfn, PhysicalMemoryMap, PhysicalPageFlags};
    use snap::compute_compound_pages;

    use crate::{Message, ProcessInfo, UpdateMessage};
    use egui_macroquad::egui::{self, RichText};
    use egui_macroquad::egui::{Color32, TextWrapMode};

    fn recompute_rgb_data(
        rgb_offsets: &mut [i8; 3],
        rgb_flag_names: &mut [String; 3],
        rgb_selector: usize,
        flags_count: usize,
    ) {
        rgb_offsets[rgb_selector] = rgb_offsets[rgb_selector].rem_euclid(flags_count as i8);
        rgb_flag_names[rgb_selector] = PhysicalPageFlags::all()
            .iter_names()
            .nth(rgb_offsets[rgb_selector] as usize)
            .unwrap()
            .0
            .to_string();
    }

    fn gen_image(
        default_img: &Image,
        memory_segments: &[(Pfn, Pfn, Vec<PhysicalPageFlags>)],
        iomem: &[PhysicalMemoryMap],
        order: u8,
        r_flag: PhysicalPageFlags,
        g_flag: PhysicalPageFlags,
        b_flag: PhysicalPageFlags,
    ) -> Image {
        let mut img = default_img.clone();
        let page_size = 4096; // TODO

        for (start_pfn, end_pfn, flags) in memory_segments.iter() {
            assert_eq!(end_pfn.0 - start_pfn.0, flags.len() as u64);

            for (pfn, &flag) in (start_pfn.0..end_pfn.0).zip(flags.iter()) {
                let index = snap::pfn_to_index(iomem, page_size, Pfn(pfn)).unwrap();

                let pfn2 = snap::index_to_pfn(iomem, page_size, index).unwrap();
                assert_eq!(Pfn(pfn), pfn2);

                let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

                let mut c = [0u8, 0, 0];
                if flag.contains(r_flag) {
                    c[0] = 255;
                } else {
                    c[0] = 0;
                }
                if flag.contains(g_flag) {
                    c[1] = 255;
                } else {
                    c[1] = 0;
                }
                if flag.contains(b_flag) {
                    c[2] = 255;
                } else {
                    c[2] = 0;
                }

                let color = Color::from_rgba(c[0], c[1], c[2], 255);
                img.set_pixel(x as u32, y as u32, color);
            }
        }

        img
    }

    pub fn client(remote: SocketAddr) {
        macroquad::Window::new("kpageflags-viewer", async_client(remote));
    }

    async fn async_client(remote: SocketAddr) {
        let page_size = 4096;

        let mut socket =
            TcpStream::connect_timeout(&remote, std::time::Duration::from_secs(5)).unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);

        let socket_thread = thread::spawn(move || loop {
            let message = Message::recv(&mut socket).unwrap();
            tx.try_send(message).unwrap();
        });

        let mut default_img: Option<Image> = None;
        let mut img: Option<Image> = None;
        let mut texture: Option<Texture2D> = None;

        let mut zoom = 1.;
        let mut canvas_offset = Vec2::new(0., 0.);
        let canvas_size = Vec2::new(600., 600.);
        let mut _autorefresh = true;

        // TODO: move flags to server
        let flags_count = PhysicalPageFlags::all().iter().count();
        let mut rgb_offsets = [26i8, 12, 10]; // default view
        let mut rgb_flag_names = [String::new(), String::new(), String::new()];

        let mut order: Option<u8> = None;

        let mut mouse_world: Vec2;
        let mut update: Option<UpdateMessage> = None;
        let mut pfn: Option<Pfn> = None;

        #[derive(Copy, Clone, PartialEq, Eq)]
        enum DisplayTab {
            Info,
            Help,
            Stats,
        }
        let mut tab = DisplayTab::Info;

        let mut changed = false;

        'mainloop: loop {
            let chrono = Instant::now();

            let (mouse_x, mouse_y) = mouse_position();
            let (_mouse_wheel_x, mouse_wheel_y) = mouse_wheel();

            let mouse_screen = Vec2::new(mouse_x, mouse_y);
            //mouse_world = (mouse_screen - canvas_offset) / zoom / canvas_size
            //    * Vec2::new(img.width() as f32, img.height() as f32);
            mouse_world = default_img
                .as_ref()
                .map(|img| {
                    (mouse_screen - canvas_offset) / zoom / canvas_size
                        * Vec2::new(img.width() as f32, img.height() as f32)
                })
                .unwrap_or_default();

            // TODO: zoom from mouse cursor
            let zoom_factor = 1.2;
            if mouse_wheel_y > 0. {
                let mouse_on_canvas = mouse_screen - canvas_offset;
                canvas_offset = mouse_screen - mouse_on_canvas * zoom_factor;
                zoom *= zoom_factor;
            }
            if mouse_wheel_y < 0. {
                let mouse_on_canvas = mouse_screen - canvas_offset;
                canvas_offset = mouse_screen - mouse_on_canvas / zoom_factor;
                zoom /= zoom_factor;
            }

            if is_key_pressed(KeyCode::Space) {
                _autorefresh ^= true;
            }

            if let Ok(message) = rx.try_recv() {
                match message {
                    Message::FirstUpdate(message) => {
                        update = Some(message.update_message);

                        let pfns = snap::get_pfn_count(&update.as_ref().unwrap().iomem);
                        let order_f64 = (pfns as f64).log2() / 2.;
                        order = Some(order_f64.ceil() as u8);

                        default_img = Some(Image::gen_image_color(
                            2u16.pow(order.unwrap() as u32),
                            2u16.pow(order.unwrap() as u32),
                            Color::from_rgba(79, 79, 79, 255),
                        ));

                        for (start, end, _) in &update.as_ref().unwrap().memory_segments {
                            for pfn in start.0..end.0 {
                                let index = snap::pfn_to_index(
                                    &update.as_ref().unwrap().iomem,
                                    page_size,
                                    Pfn(pfn),
                                )
                                .unwrap();
                                let (x, y) =
                                    fast_hilbert::h2xy::<u64>(index.into(), order.unwrap());

                                default_img.as_mut().unwrap().set_pixel(
                                    x as u32,
                                    y as u32,
                                    Color::from_rgba(64, 64, 64, 255),
                                );
                            }
                        }

                        // first loop
                        for i in 0..3 {
                            recompute_rgb_data(
                                &mut rgb_offsets,
                                &mut rgb_flag_names,
                                i,
                                flags_count,
                            );
                        }

                        img = default_img.clone();
                        texture = Some(Texture2D::from_image(img.as_ref().unwrap()));
                    }
                    Message::Update(message) => {
                        update = Some(message);
                        // TODO: update image in egui
                        changed = true;
                    }
                    Message::Finish => {
                        break 'mainloop;
                    }
                }
            }

            clear_background(DARKGRAY);

            let r_flag = PhysicalPageFlags::all()
                .iter()
                .nth(rgb_offsets[0] as usize)
                .unwrap();
            let g_flag = PhysicalPageFlags::all()
                .iter()
                .nth(rgb_offsets[1] as usize)
                .unwrap();
            let b_flag = PhysicalPageFlags::all()
                .iter()
                .nth(rgb_offsets[2] as usize)
                .unwrap();

            if changed {
                img = Some(gen_image(
                    default_img.as_ref().unwrap(),
                    &update.as_ref().unwrap().memory_segments,
                    &update.as_ref().unwrap().iomem,
                    order.unwrap(),
                    r_flag,
                    g_flag,
                    b_flag,
                ));
                texture = Some(Texture2D::from_image(img.as_ref().unwrap()));
            }

            if img.is_none() {
                // wait for first image
                next_frame().await;
                continue;
            }

            if zoom > 1. {
                texture.as_ref().unwrap().set_filter(FilterMode::Nearest);
            } else {
                texture.as_ref().unwrap().set_filter(FilterMode::Linear);
            }
            let params = DrawTextureParams {
                dest_size: Some(canvas_size * zoom),
                ..Default::default()
            };
            draw_texture_ex(
                texture.as_ref().unwrap(),
                canvas_offset.x,
                canvas_offset.y,
                WHITE,
                params,
            );
            let _elapsed = chrono.elapsed();

            egui_macroquad::ui(|egui_ctx| {
                egui_macroquad::egui::Window::new("kpageflags").show(egui_ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut tab, DisplayTab::Info, "Info");
                        ui.selectable_value(&mut tab, DisplayTab::Stats, "Stats");
                        ui.selectable_value(&mut tab, DisplayTab::Help, "Help");
                    });

                    match tab {
                        DisplayTab::Info => {
                            for ((i, &color), &color_name) in (0..3)
                                .zip(&[Color32::RED, Color32::GREEN, Color32::BLUE])
                                .zip(&["Red", "Green", "Blue"])
                            {
                                egui::ComboBox::from_label(color_name)
                                    .selected_text(
                                        RichText::new(
                                            PhysicalPageFlags::all()
                                                .iter_names()
                                                .nth(rgb_offsets[i] as usize)
                                                .map(|(name, _)| name)
                                                .unwrap(),
                                        )
                                        .color(color),
                                    )
                                    .show_ui(ui, |ui| {
                                        ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                                        ui.set_min_width(60.0);

                                        for (idx, flag_name) in PhysicalPageFlags::all()
                                            .iter_names()
                                            .map(|(flag_name, _)| flag_name)
                                            .enumerate()
                                        {
                                            if ui
                                                .selectable_value(
                                                    &mut rgb_offsets[i],
                                                    idx as i8,
                                                    flag_name,
                                                )
                                                .changed()
                                            {
                                                // update image
                                            }
                                        }
                                    });
                                //ui.label("");
                            }

                            ui.separator();

                            ui.label(format!("pfn: {:?}", pfn.map(|pfn| pfn.0)));

                            if let Some(pfn) = pfn {
                                // mouse is over canvas AND RAM
                                let mut flags: Option<PhysicalPageFlags> = None;
                                for (pfn_start, pfn_end, segment_flags) in
                                    &update.as_ref().unwrap().memory_segments
                                {
                                    if pfn >= *pfn_start && pfn < *pfn_end {
                                        flags = segment_flags
                                            .get((pfn.0 - pfn_start.0) as usize)
                                            .copied();
                                    }
                                }

                                let flags_text: String = if let Some(flags) = flags {
                                    flags.iter_names().map(|(flag_name, _)| flag_name).join(" ")
                                } else {
                                    // TODO: should be unreachable somehow
                                    "NOT IN RAM?".into()
                                };
                                ui.label(format!("flags: {}", flags_text));

                                let processes: Vec<&ProcessInfo> = update
                                    .as_ref()
                                    .unwrap()
                                    .processes_info
                                    .iter()
                                    .filter(|proc_info| proc_info.pfns.contains(&pfn))
                                    .collect();

                                ui.separator();

                                use egui_extras::{Column, TableBuilder};
                                let table = TableBuilder::new(ui)
                                    .striped(true)
                                    .resizable(false)
                                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                                    .column(Column::auto())
                                    .column(Column::remainder())
                                    .min_scrolled_height(0.0);

                                table
                                    .header(20.0, |mut header| {
                                        header.col(|ui| {
                                            ui.strong("PID");
                                        });
                                        header.col(|ui| {
                                            ui.strong("exe");
                                        });
                                    })
                                    .body(|mut body| {
                                        for proc in &processes {
                                            body.row(20.0, |mut row| {
                                                row.col(|ui| {
                                                    ui.label(format!("{}", proc.pid));
                                                });
                                                row.col(|ui| {
                                                    ui.label(proc.exe.to_string_lossy());
                                                });
                                            });
                                        }
                                    });
                            }

                            // see https://github.com/optozorax/egui-macroquad/issues/26
                            if mouse_world.x >= Vec2::ZERO.x
                                && mouse_world.y >= Vec2::ZERO.y
                                && mouse_world.x < img.as_ref().unwrap().width() as f32
                                && mouse_world.y < img.as_ref().unwrap().height() as f32
                                && !egui_ctx.wants_pointer_input()
                                && !egui_ctx.is_pointer_over_area()
                            {
                                // mouse is over a canvas

                                if macroquad::input::is_mouse_button_down(
                                    macroquad::miniquad::MouseButton::Left,
                                ) {
                                    let index = fast_hilbert::xy2h::<u64>(
                                        mouse_world.x as u64,
                                        mouse_world.y as u64,
                                        order.unwrap(),
                                    ) as u64;
                                    pfn = snap::index_to_pfn(
                                        &update.as_ref().unwrap().iomem,
                                        page_size,
                                        index,
                                    );
                                }
                            }
                        }
                        DisplayTab::Help => {
                            ui.label("Left click to select page");
                        }
                        DisplayTab::Stats => {
                            // memory_segments[2] == normal zone
                            let stats = compute_compound_pages(
                                &update.as_ref().unwrap().memory_segments[2].2,
                            );

                            use egui_extras::{Column, TableBuilder};
                            let table = TableBuilder::new(ui)
                                .striped(true)
                                .resizable(false)
                                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                                .column(Column::auto())
                                .column(Column::auto())
                                .column(Column::remainder())
                                .min_scrolled_height(0.0);

                            table
                                .header(20.0, |mut header| {
                                    header.col(|ui| {
                                        ui.strong("Flag");
                                    });
                                    header.col(|ui| {
                                        ui.strong("Pages");
                                    });
                                    header.col(|ui| {
                                        ui.strong("Size");
                                    });
                                })
                                .body(|mut body| {
                                    for ((name, _flag), stat) in
                                        PhysicalPageFlags::all().iter_names().zip(&stats)
                                    {
                                        body.row(20.0, |mut row| {
                                            row.col(|ui| {
                                                ui.label(name);
                                            });
                                            row.col(|ui| {
                                                ui.label(format!("{}", stat));
                                            });
                                            row.col(|ui| {
                                                ui.label(format!(
                                                    "{} MiB",
                                                    stat * 4096 / 1024 / 1024
                                                ));
                                            });
                                        });
                                    }
                                });
                        }
                    }
                });
            });

            // Draw things before egui

            egui_macroquad::draw();

            //thread::sleep(Duration::from_millis(1));
            next_frame().await;

            let _elapsed = chrono.elapsed();
            //dbg!(elapsed);

            changed = false;
        }

        socket_thread.join().unwrap();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match args[..] {
        [_, "client", remote] => {
            // TODO: resolve names
            let remote: SocketAddr = remote.parse().unwrap();
            client::client(remote);
        }
        [_, "server", socket] => {
            let port: SocketAddr = socket.parse().unwrap();
            #[cfg(unix)]
            server::server(port);
        }
        [me] => {
            let mut server = Command::new(me)
                .args(vec!["server", "127.0.0.1:10000"])
                .spawn()
                .expect("Can't spawn server");
            thread::sleep(Duration::from_millis(10));

            let mut client = Command::new(me)
                .args(vec!["client", "127.0.0.1:10000"])
                .spawn()
                .expect("Can't spawn client");

            let mut client_exited = false;
            let mut server_exited = false;
            loop {
                if let Ok(Some(_)) = server.try_wait() {
                    server_exited = true;
                }
                if let Ok(Some(_)) = client.try_wait() {
                    client_exited = true;
                }
                if client_exited && server_exited {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        _ => panic!("Unknown args {:?}", args),
    }
}

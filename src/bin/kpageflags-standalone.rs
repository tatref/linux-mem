// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

use std::collections::HashSet;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self};
use std::time::Duration;

use itertools::Itertools;
use macroquad::prelude::*;
use procfs::process::{MMapPath, Pfn, Process};
use procfs::{prelude::*, KPageFlags};
use procfs::{PhysicalMemoryMap, PhysicalPageFlags};

fn get_process_pfns(process: &Process) -> Result<HashSet<Pfn>, Box<dyn std::error::Error>> {
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

fn get_all_processes_pfns() -> Vec<(Process, HashSet<Pfn>)> {
    let processes: Vec<(Process, HashSet<Pfn>)> = procfs::process::all_processes()
        .unwrap()
        .filter_map(|res| res.ok())
        .filter_map(|p| match get_process_pfns(&p) {
            Ok(pfns) => Some((p, pfns)),
            Err(_) => None,
        })
        .collect();

    processes
}

fn get_segments(
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
    default_img: Image,
    memory_segments: &[(Pfn, Pfn, Vec<PhysicalPageFlags>)],
    iomem: &[PhysicalMemoryMap],
    order: u8,
    r_flag: PhysicalPageFlags,
    g_flag: PhysicalPageFlags,
    b_flag: PhysicalPageFlags,
) -> Image {
    let mut img = default_img.clone();
    let page_size = procfs::page_size();

    for (start_pfn, end_pfn, flags) in memory_segments.iter() {
        assert_eq!(end_pfn.0 - start_pfn.0, flags.len() as u64);

        for (pfn, &flag) in (start_pfn.0..end_pfn.0).zip(flags.iter()) {
            let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();

            let pfn2 = snap::index_to_pfn(&iomem, page_size, index).unwrap();
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

fn window_conf() -> Conf {
    Conf {
        window_title: "KPageFlags-Viewer".to_owned(),
        window_width: 1024,
        window_height: 600,
        //fullscreen: true,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let iomem: Vec<PhysicalMemoryMap> = procfs::iomem()
        .unwrap()
        .iter()
        .filter_map(|(ident, map)| if *ident == 0 { Some(map.clone()) } else { None })
        .filter(|map| map.name == "System RAM")
        .collect();
    let page_size = procfs::page_size();

    let mut kpageflags = procfs::KPageFlags::new().unwrap();
    let flags_count = PhysicalPageFlags::all().iter().count();

    let pfns = snap::get_pfn_count(&iomem);
    let order = (pfns as f64).log2() / 2.;
    let order = order.ceil() as u8;

    let mut default_img: Image = Image::gen_image_color(
        2u16.pow(order as u32),
        2u16.pow(order as u32),
        Color::from_rgba(79, 79, 79, 255),
    );

    for map in iomem.iter() {
        let (start, end) = map.get_range().get();
        for pfn in start.0..end.0 {
            let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();
            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

            default_img.set_pixel(x as u32, y as u32, Color::from_rgba(64, 64, 64, 255));
        }
    }

    let target_update_interval = 1.;
    let mut update_elapsed = 0.;
    let mut autorefresh = true;

    let mut img = default_img.clone();
    let mut zoom = 1.;

    let mut canvas_offset = Vec2::new(0., 0.);
    let canvas_size = Vec2::new(600., 600.);

    let mut rgb_offsets = [26i8, 12, 10]; // default view
    let mut rgb_selector = 0;
    let mut rgb_flag_names = [String::new(), String::new(), String::new()];

    let mut mouse_world: Vec2;
    let mut segments = get_segments(&iomem, &mut kpageflags);
    let mut all_processes: Vec<(Process, HashSet<Pfn>)> = get_all_processes_pfns();

    // first loop
    for i in 0..3 {
        recompute_rgb_data(&mut rgb_offsets, &mut rgb_flag_names, i, flags_count);
    }

    struct ThreadData {
        memory_segments: Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)>,
        processes_pfns: Vec<(Process, HashSet<Pfn>)>,
    }
    let (tx, rx): (SyncSender<ThreadData>, Receiver<ThreadData>) = mpsc::sync_channel(1);

    let _worker_thread = thread::spawn(move || {
        let iomem: Vec<PhysicalMemoryMap> = procfs::iomem()
            .unwrap()
            .iter()
            .filter_map(|(ident, map)| if *ident == 0 { Some(map.clone()) } else { None })
            .filter(|map| map.name == "System RAM")
            .collect();
        let mut kpageflags = procfs::KPageFlags::new().unwrap();

        loop {
            let chrono = std::time::Instant::now();
            let memory_segments = get_segments(&iomem, &mut kpageflags);
            let processes_pfns = get_all_processes_pfns();
            let elapsed = chrono.elapsed().as_secs_f64();

            tx.send(ThreadData {
                memory_segments,
                processes_pfns,
            })
            .expect("Can't send thread data");

            let sleep = (target_update_interval - elapsed).max(0.);
            thread::sleep(Duration::from_secs_f64(sleep));
        }
    });

    let (tx_image, rx_image): (SyncSender<Image>, Receiver<Image>) = mpsc::sync_channel(1);
    type ImageInput = (
        Image,
        Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)>,
        Vec<PhysicalMemoryMap>,
        u8,
        PhysicalPageFlags,
        PhysicalPageFlags,
        PhysicalPageFlags,
    );
    let (tx_image_input, rx_image_input): (SyncSender<ImageInput>, Receiver<ImageInput>) =
        mpsc::sync_channel(1);
    let _update_image_thread = thread::spawn(move || loop {
        if let Ok((default_img, memory_segments, iomem, order, r_flag, g_flag, b_flag)) =
            rx_image_input.try_recv()
        {
            let img = gen_image(
                default_img,
                &memory_segments,
                &iomem,
                order,
                r_flag,
                g_flag,
                b_flag,
            );
            tx_image.send(img).expect("Can't send image");
        }
    });

    loop {
        let (mouse_x, mouse_y) = mouse_position();
        let (_mouse_wheel_x, mouse_wheel_y) = mouse_wheel();

        let mouse_screen = Vec2::new(mouse_x, mouse_y);
        mouse_world = (mouse_screen - canvas_offset) / zoom / canvas_size
            * Vec2::new(img.width() as f32, img.height() as f32);

        // TODO: zoom from mouse cursor
        let zoom_factor = 1.2;
        if mouse_wheel_y == 1. {
            let mouse_on_canvas = mouse_screen - canvas_offset;
            canvas_offset = mouse_screen - mouse_on_canvas * zoom_factor;
            zoom *= zoom_factor;
        }
        if mouse_wheel_y == -1. {
            let mouse_on_canvas = mouse_screen - canvas_offset;
            canvas_offset = mouse_screen - mouse_on_canvas / zoom_factor;
            zoom /= zoom_factor;
        }

        if is_key_pressed(KeyCode::Space) {
            autorefresh ^= true;
        }

        // TODO: move canvas with mouse
        if is_key_pressed(KeyCode::Left) {
            rgb_offsets[rgb_selector] -= 1;

            recompute_rgb_data(
                &mut rgb_offsets,
                &mut rgb_flag_names,
                rgb_selector,
                flags_count,
            );
        }
        if is_key_pressed(KeyCode::Right) {
            rgb_offsets[rgb_selector] += 1;

            recompute_rgb_data(
                &mut rgb_offsets,
                &mut rgb_flag_names,
                rgb_selector,
                flags_count,
            );
        }
        if is_key_pressed(KeyCode::Up) {
            rgb_selector -= 1;
            rgb_selector = rgb_selector.rem_euclid(3);
        }
        if is_key_pressed(KeyCode::Down) {
            rgb_selector += 1;
            rgb_selector = rgb_selector.rem_euclid(3);
        }

        clear_background(DARKGRAY);

        if let (Ok(data), true) = (rx.try_recv(), autorefresh) {
            let update_chrono = get_time();

            (segments, all_processes) = (data.memory_segments, data.processes_pfns);

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

            // send data to generate image in different thread
            tx_image_input
                .try_send((
                    default_img.clone(),
                    segments.clone(),
                    iomem.clone(),
                    order,
                    r_flag,
                    g_flag,
                    b_flag,
                ))
                .expect("Can't send image");

            update_elapsed = get_time() - update_chrono;
            //dbg!(update_elapsed);
        }

        // try to receive image from other thread
        if let Ok(new_img) = rx_image.try_recv() {
            img = new_img;
        }
        let texture = Texture2D::from_image(&img);
        if zoom > 1. {
            texture.set_filter(FilterMode::Nearest);
        } else {
            texture.set_filter(FilterMode::Linear);
        }
        let params = DrawTextureParams {
            dest_size: Some(canvas_size * zoom),
            ..Default::default()
        };
        draw_texture_ex(&texture, canvas_offset.x, canvas_offset.y, WHITE, params);

        let right_panel_offset = 600.;
        // right panel
        draw_rectangle(
            right_panel_offset,
            0.,
            screen_width() - right_panel_offset,
            screen_height(),
            DARKGRAY,
        );

        let mut text_y = 40.;

        draw_text_ex(
            &format!(
                "Autorefresh: {}, Update time {:.1}ms",
                autorefresh,
                update_elapsed * 1000.
            ),
            right_panel_offset + 20.,
            text_y,
            TextParams::default(),
        );
        text_y += 20.;

        draw_text_ex(
            &format!("Mouse_world: {:?}", mouse_world),
            right_panel_offset + 20.,
            text_y,
            TextParams::default(),
        );
        text_y += 20.;

        // draw PFN info
        // TODO add panel check for mouse
        if mouse_world.x >= Vec2::ZERO.x
            && mouse_world.y >= Vec2::ZERO.y
            && mouse_world.x < img.width() as f32
            && mouse_world.y < img.height() as f32
        {
            // mouse is over a canvas

            let index =
                fast_hilbert::xy2h::<u64>(mouse_world.x as u64, mouse_world.y as u64, order) as u64;

            draw_text_ex(
                &format!(
                    "index: {:?}, mouse: {:?}, zoom: {:.2}",
                    index,
                    (mouse_world.x as u64, mouse_world.y as u64),
                    zoom,
                ),
                right_panel_offset + 20.,
                text_y,
                TextParams::default(),
            );
            text_y += 20.;

            // if pfn == None, we are outside of RAM, because canvas is square but memory may not fill the whole canvas
            let pfn: Option<Pfn> = snap::index_to_pfn(&iomem, page_size, index);

            let page_size = procfs::page_size();
            let is_in_ram = pfn.map(|pfn| snap::pfn_is_in_ram(&iomem, page_size, pfn).is_some());

            text_y += 20.;
            draw_text_ex(
                &format!("pfn: {:?}, is_in_ram: {:?}", pfn, is_in_ram),
                right_panel_offset + 20.,
                text_y,
                TextParams::default(),
            );
            text_y += 20.;

            if let Some(pfn) = pfn {
                // mouse is over canvas AND RAM
                let mut flags: Option<PhysicalPageFlags> = None;
                for (pfn_start, pfn_end, segment_flags) in &segments {
                    if pfn >= *pfn_start && pfn < *pfn_end {
                        flags = segment_flags.get((pfn.0 - pfn_start.0) as usize).copied();
                    }
                }

                let flags_text: String = if let Some(flags) = flags {
                    flags.iter_names().map(|(flag_name, _)| flag_name).join(" ")
                } else {
                    // TODO: should be unreachable somehow
                    "NOT IN RAM?".into()
                };

                draw_text_ex(
                    &format!("flags: {}", flags_text),
                    right_panel_offset + 20.,
                    text_y,
                    TextParams::default(),
                );
                text_y += 20.;

                let processes: Vec<&Process> = all_processes
                    .iter()
                    .filter_map(|(process, pfns)| {
                        if pfns.contains(&pfn) {
                            Some(process)
                        } else {
                            None
                        }
                    })
                    .collect();
                let processes_text = processes
                    .iter()
                    .flat_map(|p| match p.exe() {
                        Ok(exe) => Some(format!("{} ({:?})", p.pid, exe)),
                        Err(_) => None,
                    })
                    .join(", ");

                draw_text_ex(
                    &format!("procs: {}", processes_text),
                    right_panel_offset + 20.,
                    text_y,
                    TextParams::default(),
                );
                text_y += 20.;
            }
        }

        for (i, color) in (0..3).zip(["RED", "GREEN", "BLUE"]) {
            let mut params = TextParams::default();
            if i == rgb_selector {
                params.color = BLACK;
            }

            draw_text_ex(
                &format!("{}: {}", color, rgb_flag_names[i]),
                right_panel_offset + 20.,
                text_y,
                params,
            );
            text_y += 20.;
        }

        next_frame().await
    }
}

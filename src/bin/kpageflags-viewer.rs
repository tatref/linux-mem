// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

use std::collections::HashSet;

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
    let page_size = procfs::page_size();

    let iomem: Vec<PhysicalMemoryMap> = procfs::iomem()
        .unwrap()
        .iter()
        .filter_map(|(ident, map)| if *ident == 0 { Some(map.clone()) } else { None })
        .filter(|map| map.name == "System RAM")
        .collect();

    let mut kpageflags = procfs::KPageFlags::new().unwrap();
    let flags_count = PhysicalPageFlags::all().iter().count();

    let pfns = snap::get_pfn_count(&iomem, page_size);
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
            let index = snap::pfn_to_index(&iomem, Pfn(pfn)).unwrap();
            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

            default_img.set_pixel(x as u32, y as u32, Color::from_rgba(64, 64, 64, 255));
        }
    }

    let mut last_update = 0.;
    let target_update_interval = 2.;
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

    loop {
        let t = get_time();

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

        // TODO: fix timestep
        if t > last_update + target_update_interval && autorefresh {
            let update_chrono = get_time();
            last_update = update_chrono;

            let get_segments_chrono = get_time();
            segments = get_segments(&iomem, &mut kpageflags); // expensive!
            let get_segments_elapsed = get_time() - get_segments_chrono;
            //dbg!(get_segments_elapsed);

            all_processes = get_all_processes_pfns(); // expensive!

            let update_image_chrono = get_time();
            for (start_pfn, end_pfn, flags) in segments.iter() {
                assert_eq!(end_pfn.0 - start_pfn.0, flags.len() as u64);

                for (pfn, &flag) in (start_pfn.0..end_pfn.0).zip(flags.iter()) {
                    let index = snap::pfn_to_index(&iomem, Pfn(pfn)).unwrap();
                    let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

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
            let update_image_elapsed = get_time() - update_image_chrono;
            //dbg!(update_image_elapsed);

            update_elapsed = get_time() - update_chrono;
            //dbg!(update_elapsed);
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

        draw_text_ex(
            &format!(
                "Autorefresh: {}, Update time {:.1}ms",
                autorefresh,
                update_elapsed * 1000.
            ),
            right_panel_offset + 20.,
            40.0,
            TextParams::default(),
        );

        draw_text_ex(
            &format!("Mouse_world: {:?}", mouse_world),
            right_panel_offset + 20.,
            60.0,
            TextParams::default(),
        );

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

            // if pfn == None, we are outside of RAM, because canvas is square but memory may not fill the whole canvas
            let pfn: Option<Pfn> = snap::index_to_pfn(&iomem, index);

            draw_text_ex(
                &format!(
                    "index: {:?}, mouse: {:?}, zoom: {:.2}, pfn: {:?}",
                    index,
                    (mouse_world.x as u64, mouse_world.y as u64),
                    zoom,
                    pfn
                ),
                right_panel_offset + 20.,
                80.0,
                TextParams::default(),
            );

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
                    160.0,
                    TextParams::default(),
                );

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
                    180.0,
                    TextParams::default(),
                );
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
                100.0 + i as f32 * 20.,
                params,
            );
        }

        next_frame().await
    }
}

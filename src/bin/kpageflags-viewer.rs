// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

//use image::{ImageBuffer, Rgb, RgbImage};
use macroquad::prelude::*;
use procfs::process::Pfn;
use procfs::{prelude::*, KPageFlags};
use procfs::{PhysicalMemoryMap, PhysicalPageFlags};

fn get_segments(
    iomem: &Vec<PhysicalMemoryMap>,
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
        Color::from_rgba(0, 0, 0, 255),
    );

    for map in iomem.iter() {
        let (start, end) = map.get_range().get();
        for pfn in start.0..end.0 {
            let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();
            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

            default_img.set_pixel(x as u32, y as u32, Color::from_rgba(64, 64, 64, 255));
        }
    }

    let mut last_update = 0.;
    let target_update_interval = 1.;
    let mut update_elapsed = 0.;

    let mut img = default_img.clone();
    let mut zoom = 1.;

    let mut canvas_offset = Vec2::new(0., 0.);

    let mut rgb_offsets = [0i8; 3];
    let mut rgb_selector = 0;
    let mut rgb_flag_names = [String::new(), String::new(), String::new()];

    loop {
        let t = get_time();

        let (mouse_x, mouse_y) = mouse_position();
        let (_mouse_wheel_x, mouse_wheel_y) = mouse_wheel();

        let mouse_screen = Vec2::new(mouse_x, mouse_y);
        let mouse_world = (mouse_screen - canvas_offset) / zoom;

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

        // TODO: move canvas with mouse

        if is_key_pressed(KeyCode::Left) {
            rgb_offsets[rgb_selector] -= 1;
            rgb_offsets[rgb_selector] = rgb_offsets[rgb_selector].rem_euclid(flags_count as i8);
            rgb_flag_names[rgb_selector] = PhysicalPageFlags::all()
                .iter_names()
                .nth(rgb_offsets[rgb_selector] as usize)
                .unwrap()
                .0
                .to_string();
        }
        if is_key_pressed(KeyCode::Right) {
            rgb_offsets[rgb_selector] += 1;
            rgb_offsets[rgb_selector] = rgb_offsets[rgb_selector].rem_euclid(flags_count as i8);
            rgb_flag_names[rgb_selector] = PhysicalPageFlags::all()
                .iter_names()
                .nth(rgb_offsets[rgb_selector] as usize)
                .unwrap()
                .0
                .to_string();
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
        if t > last_update + target_update_interval {
            let update_chrono = get_time();
            last_update = update_chrono;

            let get_segments_chrono = get_time();
            let segments = get_segments(&iomem, &mut kpageflags);
            let get_segments_elapsed = get_time() - get_segments_chrono;
            //dbg!(get_segments_elapsed);

            let update_image_chrono = get_time();
            for (start_pfn, end_pfn, flags) in segments.iter() {
                assert_eq!(end_pfn.0 - start_pfn.0, flags.len() as u64);

                for (pfn, &flag) in (start_pfn.0..end_pfn.0).zip(flags.iter()) {
                    let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();
                    let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

                    let color_on = Color::from_rgba(0, 200, 200, 255);
                    let color_off = Color::from_rgba(0, 100, 100, 255);

                    let mut c = [0u8, 0, 0];

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

                    //if flag.contains(PhysicalPageFlags::BUDDY) {
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

        let canvas_size = Vec2::new(600., 600.);
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
            &format!("Update time {:.1}ms", update_elapsed * 1000.),
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
        if mouse_world.x <= right_panel_offset && mouse_world.y < canvas_size.y {
            let index =
                fast_hilbert::xy2h::<u64>(mouse_world.x as u64, mouse_world.y as u64, order);
            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);
            assert_eq!(mouse_world.x as u64, x);
            assert_eq!(mouse_world.y as u64, y);

            // TODO: index_to_pfn
            //let pfn: Pfn = snap::index_to_pfn(&iomem, page_size, index).unwrap();

            draw_text_ex(
                &format!("index: {:?}", index),
                right_panel_offset + 20.,
                80.0,
                TextParams::default(),
            );
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

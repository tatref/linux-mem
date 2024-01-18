// Display physical pages flags
// Uses /proc/iomem and /proc/kpageflags
//

//use image::{ImageBuffer, Rgb, RgbImage};
use procfs::{prelude::*, KPageFlags};
use procfs::process::Pfn;
use procfs::{PhysicalMemoryMap, PhysicalPageFlags};
use macroquad::prelude::*;


fn get_segments(iomem: &Vec<PhysicalMemoryMap>, kpageflags: &mut KPageFlags) -> Vec<(Pfn, Pfn, Vec<PhysicalPageFlags>)>{
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



#[macroquad::main("KPageFlags")]
async fn main() {
    let page_size = procfs::page_size();

    let iomem: Vec<PhysicalMemoryMap> = procfs::iomem()
        .unwrap()
        .iter()
        .filter_map(|(ident, map)| if *ident == 0 { Some(map.clone()) } else { None })
        .filter(|map| map.name == "System RAM")
        .collect();

    let mut kpageflags = procfs::KPageFlags::new().unwrap();

    let pfns = snap::get_pfn_count(&iomem, page_size);
    let order = (pfns as f64).log2() / 2.;
    let order = order.ceil() as u8;


    let mut default_img: Image =
        Image::gen_image_color(2u16.pow(order as u32), 2u16.pow(order as u32), Color::from_rgba(0, 0, 0, 255));

    for map in iomem.iter().filter(|map| map.name == "System RAM") {
        let (start, end) = map.get_range().get();
        for pfn in start.0..end.0 {
            let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();
            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

            if let Some(_idx) = snap::pfn_is_in_ram(&iomem, page_size, Pfn(pfn)) {
                default_img.set_pixel(x as u32, y as u32, Color::from_rgba(64, 64, 64, 255));
            }
        }
    }

    //for current_flag in PhysicalPageFlags::all().iter_names() {
    //    println!("{}", current_flag.0.to_lowercase());
    //    let mut img = default_img.clone();

    //    for (start_pfn, end_pfn, flags) in segments.iter() {
    //        assert_eq!(end_pfn.0 - start_pfn.0, flags.len() as u64);

    //        for (pfn, &flag) in (start_pfn.0..end_pfn.0).zip(flags.iter()) {
    //            let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();
    //            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

    //            let color_on = Color::from_rgba(0, 200, 200, 255);
    //            let color_off = Color::from_rgba(0, 100, 100, 255);

    //            if flag.contains(current_flag.1) {
    //                img.set_pixel(x as u32, y as u32, color_on);
    //            } else {
    //                img.set_pixel(x as u32, y as u32, color_off);
    //            }
    //        }
    //    }

    //}

//    let font = load_ttf_font("./DancingScriptRegular.ttf")
//        .await
//        .unwrap();

    let mut last_update = 0;

    loop {
        let t = get_time();

        let segments = get_segments(&iomem, &mut kpageflags);
        let mut img = default_img.clone();

        let start = get_time();
        for (start_pfn, end_pfn, flags) in segments.iter() {
            assert_eq!(end_pfn.0 - start_pfn.0, flags.len() as u64);

            for (pfn, &flag) in (start_pfn.0..end_pfn.0).zip(flags.iter()) {
                let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();
                let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

                let color_on = Color::from_rgba(0, 200, 200, 255);
                let color_off = Color::from_rgba(0, 100, 100, 255);

                if flag.contains(PhysicalPageFlags::MMAP) {
                    img.set_pixel(x as u32, y as u32, color_on);
                } else {
                    img.set_pixel(x as u32, y as u32, color_off);
                }
            }
        }
        let duration = get_time() - start;

        let texture = Texture2D::from_image(&img);
        clear_background(LIGHTGRAY);
        let params = DrawTextureParams {
            dest_size: Some(Vec2::new(600., 600.)),
            ..Default::default()
        };
        draw_texture_ex(&texture, 0., 0., WHITE, params);
        //draw_text_ex(&format!("{:.2}s", t), 620.0, 20.0, TextParams::default());
        draw_text_ex(&format!("Update time {:.2}s", duration), 620.0, 40.0, TextParams::default());
        next_frame().await
    }
}

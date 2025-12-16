use ab_glyph::{FontRef, PxScale};
use colorgrad::Gradient;
use image::{ImageBuffer, Rgb, RgbImage};
use procfs::{process::Pfn, PhysicalMemoryMap, WithCurrentSystemInfo};

fn main() {
    let page_size = procfs::page_size();

    let iomem: Vec<PhysicalMemoryMap> = procfs::iomem()
        .unwrap()
        .iter()
        .filter_map(|(ident, map)| {
            if *ident == 0 && map.name == "System RAM" {
                Some(map.clone())
            } else {
                None
            }
        })
        .collect();

    let pfns = snap::get_pfn_count(&iomem);
    dbg!(pfns);
    let order = (pfns as f64).log2() / 2.;
    let order = order.ceil() as u8;
    dbg!(order);

    let legend_offset = 1000;

    let draw_square = 2u32.pow(order as u32);
    let mut img: RgbImage = ImageBuffer::new(draw_square + legend_offset, draw_square);
    dbg!(img.dimensions());

    let grad = colorgrad::preset::rainbow();

    let empty_color = Rgb([128, 128, 128]);
    for x in 0..draw_square {
        for y in 0..draw_square {
            let px = img.get_pixel_mut(x, y);
            *px = empty_color;
        }
    }

    let segments_count = iomem.len();
    for (segment_index, segment) in iomem.iter().enumerate() {
        println!(
            "{} {:x}-{:x}: {} MiB",
            &segment.name,
            segment.address.0,
            segment.address.1,
            (segment.address.1 - segment.address.0) / 1024 / 1024
        );

        let (start_pfn, end_pfn) = segment.get_range().get();
        for pfn in start_pfn.0..end_pfn.0 {
            if pfn == 0 {
                continue;
            }
            let index = snap::pfn_to_index(&iomem, page_size, Pfn(pfn)).unwrap();
            //let x = index % draw_square as u64;
            //let y = index / draw_square as u64;
            let (x, y) = fast_hilbert::h2xy::<u64>(index.into(), order);

            let color = grad
                .at(segment_index as f32 / segments_count as f32)
                .to_linear_rgba_u8();
            let pixel = Rgb([color[0], color[1], color[2]]);

            img.put_pixel(x as u32, y as u32, pixel);
        }
    }

    let font = FontRef::try_from_slice(include_bytes!(
        "../../fonts/dejavu-fonts-ttf-2.37/ttf/DejaVuSans.ttf"
    ))
    .unwrap();
    //let scale = Scale::uniform(30.);
    let scale = PxScale { x: 40., y: 40. };

    for (segment_index, segment) in iomem.iter().enumerate() {
        let size = snap::get_size(segment);
        let string_size = humansize::format_size(size, humansize::BINARY);

        let x = draw_square as i32 + scale.x as i32 * 3;
        let y = scale.x as i32 * 3 + scale.x as i32 * segment_index as i32 * 2;

        let grad_color = grad
            .at(segment_index as f32 / segments_count as f32)
            .to_linear_rgba_u8();
        let color = Rgb([grad_color[0], grad_color[1], grad_color[2]]);

        imageproc::drawing::draw_filled_ellipse_mut(
            &mut img,
            (x, y),
            scale.x as i32,
            scale.x as i32,
            color,
        );

        let text = format!("{} - {}", segment.name, string_size);
        imageproc::drawing::draw_text_mut(
            &mut img,
            Rgb([255u8, 255u8, 255u8]),
            x + 2 * scale.x as i32,
            y,
            scale,
            &font,
            &text,
        );
    }

    img.save("img.png").unwrap();
}

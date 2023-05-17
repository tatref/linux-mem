use std::fmt::format;

use colored::Colorize;
use colorgrad::Gradient;
use log::warn;
use tabled::{settings::Modify, Tabled};

#[derive(Tabled)]
pub struct TmpfsMetadata {
    /// Mount point
    pub mount_point: String,
    /// FS size in Bytes
    pub fs_size: u64,
    /// Free space in Bytes
    pub fs_used: u64,
}

pub fn format_units_MiB(value: u64) -> String {
    let format = humansize::FormatSizeOptions::from(humansize::DECIMAL)
        .fixed_at(Some(humansize::FixedAt::Mega));
    humansize::format_size(value, format)
}

pub fn display_color_grad(
    gradient: &Gradient,
    value: u64,
    max: u64,
    zero_is_gray: bool,
    format_units: bool,
) -> String {
    let s = if format_units {
        format_units_MiB(value)
    } else {
        format!("{}", value)
    };

    if value == 0 && zero_is_gray {
        return format!("{}", s.truecolor(128, 128, 128));
    }

    let t = value as f64 / max as f64;
    let color = gradient.at(t);
    let (r, g, b, _a) = color.to_linear_rgba_u8();

    format!("{}", s.truecolor(r, g, b).bold())
}

pub fn display_tmpfs() {
    println!("Scanning tmpfs...");
    let mountinfos = procfs::process::Process::myself().unwrap().mountinfo();
    if let Ok(mountinfos) = mountinfos {
        let tabled_tmpfs_metadata: Vec<TmpfsMetadata> = mountinfos
            .into_iter()
            .filter(|mountinfo| match mountinfo.fs_type.as_str() {
                "tmpfs" => true,
                _ => false,
            })
            .map(|mountinfo| {
                let mount_point = mountinfo.mount_point;
                let statvfs = nix::sys::statvfs::statvfs(&mount_point).unwrap();
                let fs_size = statvfs.block_size() * statvfs.blocks();
                let fs_free = statvfs.block_size() * statvfs.blocks_free();
                let fs_used = fs_size - fs_free;
                let tmpfs_metadata = TmpfsMetadata {
                    mount_point: mount_point.to_string_lossy().to_string(),
                    fs_size,
                    fs_used,
                };

                tmpfs_metadata
            })
            .collect();

        let max_used = tabled_tmpfs_metadata
            .iter()
            .max_by_key(|x| x.fs_used)
            .unwrap()
            .fs_used;

        let gradient = crate::get_gradient();

        let mut table = tabled::Table::new(&tabled_tmpfs_metadata);
        table.with(tabled::settings::Style::sharp());
        for (idx, record) in tabled_tmpfs_metadata.iter().enumerate() {
            let value = record.fs_used;
            let max = max_used;
            let zero_is_gray = true;

            table.with(
                // skip header
                Modify::new((idx + 1, 2)).with(tabled::settings::format::Format::content(|_s| {
                    display_color_grad(&gradient, value, max, zero_is_gray, true)
                })),
            );
            table.with(
                // skip header
                Modify::new((idx + 1, 1)).with(tabled::settings::format::Format::content(|_s| {
                    format_units_MiB(value)
                })),
            );
        }

        println!("{table}");
        println!();
    } else {
        warn!("Can't read /proc/pid/mountinfo");
    }
}

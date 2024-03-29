use log::warn;
use tabled::Tabled;

#[derive(Tabled)]
pub struct TmpfsMetadata {
    /// Mount point
    pub mount_point: String,
    /// FS size in Bytes
    #[tabled(display_with = "format_units_MiB")]
    pub fs_size: u64,
    /// Free space in Bytes
    #[tabled(display_with = "format_units_MiB")]
    pub fs_used: u64,
}

pub fn format_units_MiB(value: &u64) -> String {
    let format = humansize::FormatSizeOptions::from(humansize::DECIMAL)
        .fixed_at(Some(humansize::FixedAt::Mega));
    humansize::format_size(*value, format)
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

        let mut table = tabled::Table::new(&tabled_tmpfs_metadata);
        table.with(tabled::settings::Style::sharp());

        println!("{table}");
        println!();
    } else {
        warn!("Can't read /proc/pid/mountinfo");
    }
}

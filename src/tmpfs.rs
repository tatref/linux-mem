use std::path::PathBuf;

use tabled::Tabled;

#[derive(Tabled)]
pub struct TmpfsMetadata {
    /// Mount point
    pub mount_point: String,
    /// FS size in Bytes
    #[tabled(display_with = "display_fs_used")]
    pub fs_size: u64,
    /// Free space in Bytes
    #[tabled(display_with = "display_fs_used")]
    pub fs_used: u64,
}

fn display_fs_used(fs_used: &u64) -> String {
    let format = humansize::FormatSizeOptions::from(humansize::DECIMAL)
        .fixed_at(Some(humansize::FixedAt::Mega));

    humansize::format_size(*fs_used, format)
}

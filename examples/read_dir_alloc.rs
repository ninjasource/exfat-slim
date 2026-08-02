mod common;

use crate::common::{BLOCK_SIZE, N, asynchronous::InMemoryBlockDevice};
use exfat_slim::asynchronous::{
    directory::MAX_NAME_LEN,
    file_system::{ExFatResult, FileSystem},
};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExFatResult<(), InMemoryBlockDevice, BLOCK_SIZE> {
    env_logger::init();
    color_backtrace::install();
    info!("reading root dir:");

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, N> = FileSystem::new(io);

    let path = ""; // root dir
    let mut dir = fs.read_dir(path).await?;
    let mut name_buf = [0; MAX_NAME_LEN];
    while let Some(entry) = dir.next_entry(&mut fs, &mut name_buf).await? {
        let entry_type = if entry.metadata.is_dir() {
            "DIR"
        } else {
            "FILE"
        };
        info!(
            "{} | {} | {} bytes",
            entry.name,
            entry_type,
            entry.metadata.len()
        );
    }

    Ok(())
}

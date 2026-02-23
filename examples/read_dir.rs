mod common;

use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();
    info!("reading root dir:");

    let io = InMemoryBlockDevice::new();
    let mut io_cloned = io.clone();
    let fs = FileSystem::new(io).await?;

    let path = ""; // root dir
    let mut dir = fs.read_dir(path).await?;
    while let Some(entry) = dir.next(&mut io_cloned).await? {
        let entry_type = if entry.metadata().is_dir() {
            "DIR"
        } else {
            "FILE"
        };
        info!(
            "{} | {} | {} bytes",
            entry.file_name(),
            entry_type,
            entry.metadata().len()
        );
    }

    Ok(())
}

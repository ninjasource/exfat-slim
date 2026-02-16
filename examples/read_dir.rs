mod common;

use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();
    info!("reading root dir:");

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;

    let path = ""; // root dir
    let mut dir = fs.read_dir(&mut io, path).await?;
    while let Some(entry) = dir.next(&mut io).await? {
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

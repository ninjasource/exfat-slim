mod common;

use crate::common::{BLOCK_SIZE, N, asynchronous::InMemoryBlockDevice};
use exfat_slim::asynchronous::{ file_system::{ExFatResult, FileSystem},};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExFatResult<(), InMemoryBlockDevice, BLOCK_SIZE> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, N> = FileSystem::new(io);
    let path = "/temp2/hello2/shoe/test.txt";

    let exists = fs.exists(path).await?;
    info!("file exists: {exists}");

    info!("deleting file");
    fs.remove_file(path).await?;

    let exists = fs.exists(path).await?;
    info!("file exists: {exists}");

    Ok(())
}

mod common;

use crate::common::{BLOCK_SIZE, N, asynchronous::InMemoryBlockDevice};
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError<InMemoryBlockDevice, BLOCK_SIZE>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, N> = FileSystem::new(io);

    let path = "/temp2/foo/shoe";
    fs.create_directory(path).await?;

    let exists = fs.exists(path).await?;
    info!("created: {exists}");

    Ok(())
}

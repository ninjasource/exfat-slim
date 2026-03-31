mod common;

use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs = FileSystem::new(io);
    let path = "/temp2/hello2/shoe/test.txt";

    let exists = fs.exists(path).await?;
    info!("file exists: {exists}");

    info!("deleting file");
    fs.remove_file(path).await?;

    let exists = fs.exists(path).await?;
    info!("file exists: {exists}");

    Ok(())
}

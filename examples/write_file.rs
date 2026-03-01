mod common;
use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(io).await?;
    let path = "/temp2/test7.txt";

    // this will create any folders that do not exist
    fs.write(path, b"hello").await?;

    info!("file created: {}", fs.exists(path).await?);

    Ok(())
}

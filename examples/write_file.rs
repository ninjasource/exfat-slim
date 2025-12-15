mod common;
use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;
    let full_path = "/temp2/test7.txt";

    // this will create any folders that do not exist
    fs.write(&mut io, full_path, b"hello").await?;

    info!("file created: {}", fs.exists(&mut io, full_path).await?);

    Ok(())
}

use log::info;

mod common;
use crate::common::InMemoryBlockDevice;
use exfat_slim::{error::ExFatError, file_system::FileSystem};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();
    info!("deleting file");

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;
    fs.delete(&mut io, "/temp2/hello2/shoe/test.txt").await?;

    Ok(())
}

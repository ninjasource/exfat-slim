use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

use crate::common::asynchronous::InMemoryBlockDevice;

mod common;

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

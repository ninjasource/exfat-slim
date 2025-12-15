mod common;

use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();
    info!("reading file");

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;
    info!("{fs:?}");
    let contents = fs
        .read_to_string(&mut io, "/temp2/hello2/shoe/test.txt")
        .await?;
    info!("{contents}");

    Ok(())
}

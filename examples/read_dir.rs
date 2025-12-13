mod common;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

use crate::common::asynchronous::InMemoryBlockDevice;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();
    info!("reading root dir");

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;

    let path = "/";
    let mut list = fs.read_dir(&mut io, path).await?;
    while let Some(item) = list.next(&mut io).await? {
        info!("{:?}", item);
    }

    Ok(())
}

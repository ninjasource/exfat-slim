mod common;
use crate::common::InMemoryBlockDevice;
use exfat_slim::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();
    info!("reading directory list");

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;

    let path = ""; // or "/temp2/hello2"
    let mut list = fs.read_dir(&mut io, path).await?;
    while let Some(item) = list.next(&mut io).await? {
        info!("{:?}", item);
    }

    Ok(())
}

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

    fs.rename(&mut io, "/temp2/test6.txt", "test6x.txt").await?;
    fs.rename(&mut io, "/temp2/hello2", "hello2x").await?;

    let path = "/temp2";
    let mut list = fs.read_dir(&mut io, path).await?;
    while let Some(item) = list.next(&mut io).await? {
        info!("{:?}", item);
    }
    Ok(())
}

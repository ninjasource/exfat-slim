use log::info;

mod common;
use crate::common::InMemoryBlockDevice;
use exfat_slim::{error::ExFatError, file_system::FileSystem};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;
    let full_path = "/temp2/hello2/shoe/test1.txt";

    let exists = fs.exists(&mut io, full_path).await?;
    info!("{full_path} exists: {exists}");

    fs.write(&mut io, full_path, b"hello").await?;

    let path = "/temp2/hello2/shoe";
    let mut list = fs.read_dir(&mut io, path).await?;
    while let Some(item) = list.next(&mut io).await? {
        info!("{:?}", item);
    }
    let exists = fs.exists(&mut io, full_path).await?;
    info!("{full_path} exists: {exists}");

    Ok(())
}

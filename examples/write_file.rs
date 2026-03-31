mod common;
use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file::OpenOptions, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs = FileSystem::new(io);
    let path = "/temp2/test7.txt";

    // this will create any folders that do not exist
    fs.write(path, b"hello").await?;
    info!("file created: {}", fs.exists(path).await?);

    // an alternative way to write to a file
    let options = OpenOptions::new().create(true).write(true).append(true);
    let mut file = fs.open("test.txt", options).await?;
    file.write(&mut fs, b"hello").await?;
    info!("file created: {}", fs.exists("test.txt").await?);

    Ok(())
}

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
    let mut path = "/temp2/test7.txt";

    // this will create any folders that do not exist
    fs.write(path, b"hello").await?;
    info!("file created '{}': {}", path, fs.exists(path).await?);

    // an alternative way to write to a file
    let options = OpenOptions::new().create(true).write(true).append(true);
    path = "test.txt";
    let mut file = fs.open(path, options).await?;
    file.write(&mut fs, b"hello").await?;
    info!("file created '{}': {}", path, fs.exists(path).await?);

    Ok(())
}

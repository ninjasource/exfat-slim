mod common;
use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(io).await?;

    // copy file
    fs.copy("/temp2/test6.txt", "/temp1/test6.txt").await?;

    let s1 = fs.read_to_string("/temp2/test6.txt").await?;
    let s2 = fs.read_to_string("/temp1/test6.txt").await?;
    info!("file1: {s1} file2: {s2}");

    Ok(())
}

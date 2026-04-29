mod common;
use crate::common::{BLOCK_SIZE, N, asynchronous::InMemoryBlockDevice};
use exfat_slim::asynchronous::{
    file::OpenOptions,
    file_system::{ExFatResult, FileSystem},
};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExFatResult<(), InMemoryBlockDevice, BLOCK_SIZE> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, N> = FileSystem::new(io);
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

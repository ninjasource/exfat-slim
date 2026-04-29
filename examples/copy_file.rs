mod common;
use crate::common::{BLOCK_SIZE, N, asynchronous::InMemoryBlockDevice};
use exfat_slim::asynchronous::file_system::{ExFatResult, FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExFatResult<(), InMemoryBlockDevice, BLOCK_SIZE> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, N> = FileSystem::new(io);

    // copy file
    fs.copy("/temp2/test6.txt", "/temp1/test6.txt").await?;

    let s1 = fs.read_to_string("/temp2/test6.txt").await?;
    let s2 = fs.read_to_string("/temp1/test6.txt").await?;
    info!("file1: {s1} file2: {s2}");

    Ok(())
}

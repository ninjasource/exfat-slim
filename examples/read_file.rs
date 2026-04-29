mod common;

use crate::common::{BLOCK_SIZE, asynchronous::InMemoryBlockDevice};
use exfat_slim::asynchronous::{file_system::ExFatResult, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExFatResult<(), InMemoryBlockDevice, BLOCK_SIZE> {
    env_logger::init();
    color_backtrace::install();
    info!("reading file");

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, 4> = FileSystem::new(io);
    let contents = fs.read_to_string("/temp2/hello2/shoe/test.txt").await?;
    info!("{contents}");

    Ok(())
}

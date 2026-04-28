mod common;
use crate::common::{BLOCK_SIZE, N, asynchronous::InMemoryBlockDevice};
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

/// a rename can also be considered to be a move if it changes directories
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError<InMemoryBlockDevice, BLOCK_SIZE>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, N> = FileSystem::new(io);

    // rename file
    fs.rename("/temp2/test6.txt", "/temp2/test6x.txt").await?;

    // move folder
    fs.rename("/temp2/hello2", "/hello2").await?;

    let path = "/temp2";
    let mut list = fs.read_dir(path).await?;
    while let Some(item) = list.next_entry(&mut fs).await? {
        info!("{:?}", item);
    }

    info!(
        "directory '/hello2' exists: {}",
        fs.exists("/hello2").await?
    );
    Ok(())
}

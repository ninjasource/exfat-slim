mod common;

use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;

    fs.create_directory(&mut io, "/temp2/foo/shoe").await?;

    Ok(())
}

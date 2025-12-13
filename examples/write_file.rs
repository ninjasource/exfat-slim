mod common;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};

use crate::common::asynchronous::InMemoryBlockDevice;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;

    fs.write(&mut io, "/temp2/hello2/shoe/test1.txt", b"hello")
        .await?;

    Ok(())
}

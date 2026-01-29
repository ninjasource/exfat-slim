mod common;
use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;
    let full_path = "/temp2/test7.txt";

    let mut file = fs
        .with_options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&mut io, full_path)
        .await?;

    file.write(&mut io, b"hello").await?;
    file.write(&mut io, b" world").await?;
    file.seek(&mut io, 6).await?;
    file.write(&mut io, b"W").await?;

    let contents = fs.read_to_string(&mut io, full_path).await?;
    println!("Contents: `{contents}`");

    Ok(())
}

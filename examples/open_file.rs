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
    let path = "foo.txt";

    // create a new file and write "hello world" to it
    let mut file = fs
        .with_options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .await?;
    file.write(b"hello world").await?;

    // open file for read and read contents
    let mut file = fs.with_options().read(true).open(path).await?;
    let contents = file.read_to_string().await?;
    info!("new file: \"{contents}\"");

    // reading again should yield an empty string because we have already reached the end of the file
    let contents = file.read_to_string().await?;
    info!("read again (cursor at end of file): \"{contents}\"");

    // seek to the 6th byte in the file
    file.seek(6).await?;
    let contents = file.read_to_string().await?;
    info!("seek to byte 6 and read again: \"{contents}\"");

    // append an "!" onto the end of the file
    let mut file = fs
        .with_options()
        .write(true)
        .append(true)
        .open(path)
        .await?;
    file.write(b"!").await?;

    // attempt to read a file when read not enabled
    let contents = file.read_to_string().await;
    assert!(matches!(contents, Err(ExFatError::ReadNotEnabled)));
    info!("confirmed behaviour:  cannot read because read not enabled");

    // open file for read to get its contents
    let mut file = fs.with_options().read(true).open(path).await?;
    let contents = file.read_to_string().await?;
    info!("appended: \"{contents}\"");

    // attemt to call create_new on a file that already exists
    let file = fs
        .with_options()
        .write(true)
        .create_new(true)
        .open(path)
        .await;
    assert!(matches!(file, Err(ExFatError::AlreadyExists)));
    info!("confirmed behaviour:  cannot create new because file already exists");

    // create file without truncate - file already exists and seeks to position 0
    let mut file = fs
        .with_options()
        .create(true)
        .write(true)
        .open(path)
        .await?;
    file.write(b"12345").await?;

    // confirm expected changes
    let mut file = fs.with_options().read(true).open(path).await?;
    let contents = file.read_to_string().await?;
    info!("create file without truncate: \"{contents}\"");

    // truncate file
    let mut file = fs
        .with_options()
        .truncate(true)
        .read(true)
        .open(path)
        .await?;
    let contents = file.read_to_string().await?;
    info!("truncated: \"{contents}\"");
    let metadata = file.metadata();
    assert_eq!(0, metadata.len(), "file data length");

    // create empty read write file
    let mut file = fs
        .with_options()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
        .await?;
    file.write(b"hello").await?;
    file.seek(0).await?;
    let contents = file.read_to_string().await?;

    info!("write then read: \"{contents}\"");

    Ok(())
}

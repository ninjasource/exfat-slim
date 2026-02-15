mod common;

use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;
    let path = "foo.txt";

    // create a new file and write "hello world" to it
    let mut file = fs
        .with_options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&mut io, path)
        .await?;
    file.write(&mut io, b"hello world").await?;

    // open file for read and read contents
    let mut file = fs.with_options().read(true).open(&mut io, path).await?;
    let contents = file.read_to_string(&mut io).await?;
    info!("new file: \"{contents}\"");

    // reading again should yield an empty string because we have already reached the end of the file
    let contents = file.read_to_string(&mut io).await?;
    info!("read again (cursor at end of file): \"{contents}\"");

    // seek to the 6th byte in the file
    file.seek(&mut io, 6).await?;
    let contents = file.read_to_string(&mut io).await?;
    info!("seek to byte 6 and read again: \"{contents}\"");

    // append an "!" onto the end of the file
    let mut file = fs
        .with_options()
        .write(true)
        .append(true)
        .open(&mut io, path)
        .await?;
    file.write(&mut io, b"!").await?;

    // attempt to read a file when read not enabled
    let contents = file.read_to_string(&mut io).await;
    assert!(matches!(contents, Err(ExFatError::ReadNotEnabled)));
    info!("confirmed behaviour:  cannot read because read not enabled");

    // open file for read to get its contents
    let mut file = fs.with_options().read(true).open(&mut io, path).await?;
    let contents = file.read_to_string(&mut io).await?;
    info!("appended: \"{contents}\"");

    // attemt to call create_new on a file that already exists
    let file = fs
        .with_options()
        .write(true)
        .create_new(true)
        .open(&mut io, path)
        .await;
    assert!(matches!(file, Err(ExFatError::AlreadyExists)));
    info!("confirmed behaviour:  cannot create new because file already exists");

    // create file without truncate - file already exists and seeks to position 0
    let mut file = fs
        .with_options()
        .create(true)
        .write(true)
        .open(&mut io, path)
        .await?;
    file.write(&mut io, b"12345").await?;

    // confirm expected changes
    let mut file = fs.with_options().read(true).open(&mut io, path).await?;
    let contents = file.read_to_string(&mut io).await?;
    info!("create file without truncate: \"{contents}\"");

    // truncate file
    let mut file = fs
        .with_options()
        .truncate(true)
        .read(true)
        .open(&mut io, path)
        .await?;
    let contents = file.read_to_string(&mut io).await?;
    info!("truncated: \"{contents}\"");
    assert_eq!(0, file.details.data_length, "file data length");
    assert_eq!(0, file.details.valid_data_length, "file valid data length");

    // create empty read write file
    let mut file = fs
        .with_options()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&mut io, path)
        .await?;
    file.write(&mut io, b"hello").await?;
    file.seek(&mut io, 0).await?;
    let contents = file.read_to_string(&mut io).await?;
    info!("write then read: \"{contents}\"");

    Ok(())
}

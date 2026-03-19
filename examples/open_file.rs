mod common;

use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file::OpenBuilder, file_system::FileSystem};
use log::info;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs = FileSystem::new(io).await?;
    let path = "foo.txt";

    // create a new file and write "hello world" to it
    let options = OpenBuilder::new()
        .write(true)
        .create(true)
        .truncate(true)
        .build();
    let mut file = fs.open(path, options).await?;
    file.write(&mut fs, b"hello world").await?;

    // open file for read and read contents
    let options = OpenBuilder::new().read(true).build();
    let mut file = fs.open(path, options).await?;
    let contents = file.read_to_string(&mut fs).await?;
    info!("new file: \"{contents}\"");

    // reading again should yield an empty string because we have already reached the end of the file
    let contents = file.read_to_string(&mut fs).await?;
    info!("read again (cursor at end of file): \"{contents}\"");

    // seek to the 6th byte in the file
    file.seek(&mut fs, 6).await?;
    let contents = file.read_to_string(&mut fs).await?;
    info!("seek to byte 6 and read again: \"{contents}\"");

    // append an "!" onto the end of the file
    let options = OpenBuilder::new().write(true).append(true).build();
    let mut file = fs.open(path, options).await?;
    file.write(&mut fs, b"!").await?;

    // attempt to read a file when read not enabled
    let contents = file.read_to_string(&mut fs).await;
    assert!(matches!(contents, Err(ExFatError::ReadNotEnabled)));
    info!("confirmed behaviour:  cannot read because read not enabled");

    // open file for read to get its contents
    let options = OpenBuilder::new().read(true).build();
    let mut file = fs.open(path, options).await?;
    let contents = file.read_to_string(&mut fs).await?;
    info!("appended: \"{contents}\"");

    // attemt to call create_new on a file that already exists
    let options = OpenBuilder::new().write(true).create_new(true).build();
    let file = fs.open(path, options).await;
    assert!(matches!(file, Err(ExFatError::AlreadyExists)));
    info!("confirmed behaviour:  cannot create new because file already exists");

    // create file without truncate - file already exists and seeks to position 0
    let options = OpenBuilder::new().write(true).create(true).build();
    let mut file = fs.open(path, options).await?;
    file.write(&mut fs, b"12345").await?;

    // confirm expected changes
    let options = OpenBuilder::new().read(true).build();
    let mut file = fs.open(path, options).await?;
    let contents = file.read_to_string(&mut fs).await?;
    info!("create file without truncate: \"{contents}\"");

    // truncate file
    let options = OpenBuilder::new().read(true).truncate(true).build();
    let mut file = fs.open(path, options).await?;
    let contents = file.read_to_string(&mut fs).await?;
    info!("truncated: \"{contents}\"");
    let metadata = file.metadata();
    assert_eq!(0, metadata.len(), "file data length");

    // create empty read write file
    let options = OpenBuilder::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .build();
    let mut file = fs.open(path, options).await?;
    file.write(&mut fs, b"hello").await?;
    file.seek(&mut fs, 0).await?;
    let contents = file.read_to_string(&mut fs).await?;
    info!("write then read: \"{contents}\"");

    Ok(())
}

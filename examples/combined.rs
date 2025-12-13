use log::info;

mod common;
use crate::common::InMemoryBlockDevice;
use exfat_slim::{error::ExFatError, file_system::FileSystem};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();

    // read the boot sector of the file system
    info!("reading boot sector of file system");
    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;

    //let directory = "/bubble/gum";
    //let full_path = format!("{directory}/blue.txt");
    let directory = "/temp2/hello2/shoe";
    let full_path = format!("{directory}/test1.txt");

    // confirm that the directory does not yet exist
    info!("checking if directory exists");
    let exists = fs.exists(&mut io, directory).await?;
    info!("{directory} exists: {exists}");

    // confirm that the file does not yet exist
    info!("checking if file exists");
    let exists = fs.exists(&mut io, &full_path).await?;
    info!("{full_path} exists: {exists}");

    // write bytes to a file
    info!("writing bytes to file");
    fs.write(&mut io, &full_path, b"hello").await?;
    info!("wrote file to disk");

    // confirm that the directory exists
    info!("checking if directory exists");
    let exists = fs.exists(&mut io, directory).await?;
    info!("{directory} directory exists: {exists}");

    // confirm that the file exists
    info!("checking if file exists");
    let exists = fs.exists(&mut io, &full_path).await?;
    info!("{full_path} file exists: {exists}");

    // create an empty directory
    //info!("creating empty directory: my_dir");
    //fs.create_directory(&mut io, &format!("{directory}/my_dir"))
    //    .await?;

    // confirm that the file was created in the directory
    info!("list entries in directory {directory}");
    let mut list = fs.read_dir(&mut io, directory).await?;
    while let Some(item) = list.next(&mut io).await? {
        info!("{:?}", item);
    }

    // confirm that the file exists
    let exists = fs.exists(&mut io, &full_path).await?;
    info!("{full_path} file exists: {exists}");

    // confirm that the bytes were written to the file
    let contents = fs.read_to_string(&mut io, &full_path).await?;
    info!("contents: {contents}");

    Ok(())
}

mod common;

use crate::common::{BLOCK_SIZE, N, blocking::InMemoryBlockDevice};
use exfat_slim::blocking::{error::ExFatError, file_system::FileSystem};
use log::info;

fn main() -> Result<(), ExFatError<InMemoryBlockDevice, BLOCK_SIZE>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, N> = FileSystem::new(io);
    let path = "/temp2/hello2/shoe/test.txt";

    let exists = fs.exists(path)?;
    info!("file exists: {exists}");

    info!("deleting file");
    fs.remove_file(path)?;

    let exists = fs.exists(path)?;
    info!("file exists: {exists}");

    Ok(())
}

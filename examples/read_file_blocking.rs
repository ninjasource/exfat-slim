mod common;

use crate::common::blocking::InMemoryBlockDevice;
use exfat_slim::blocking::{error::ExFatError, file_system::FileSystem};
use log::info;

fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();
    info!("reading file");

    let io = InMemoryBlockDevice::new();
    let mut fs = FileSystem::new(io);
    let contents = fs.read_to_string("/temp2/hello2/shoe/test.txt")?;
    info!("{contents}");

    Ok(())
}

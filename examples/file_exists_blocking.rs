mod common;

use crate::common::blocking::InMemoryBlockDevice;
use exfat_slim::blocking::{error::ExFatError, file_system::FileSystem};
use log::info;

// in order to use this library in a blocking way just use the `blocking` module above instead of the `asynchronous` one
fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(io)?;
    let exists = fs.exists("/temp2/hello2/shoe/test.txt")?;
    info!("exists: {exists}");

    Ok(())
}

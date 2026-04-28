mod common;

use crate::common::{BLOCK_SIZE, N, blocking::InMemoryBlockDevice};
use exfat_slim::blocking::{error::ExFatError, file_system::FileSystem};
use log::info;

// in order to use this library in a blocking way just use the `blocking` module above instead of the `asynchronous` one
fn main() -> Result<(), ExFatError<InMemoryBlockDevice, BLOCK_SIZE>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, N> = FileSystem::new(io);
    let exists = fs.exists("/temp2/hello2/shoe/test.txt")?;
    info!("exists: {exists}");

    Ok(())
}

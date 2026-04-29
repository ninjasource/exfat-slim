mod common;

use crate::common::{BLOCK_SIZE, N, blocking::InMemoryBlockDevice};
use exfat_slim::blocking::{ file_system::{ExFatResult, FileSystem},};
use log::info;

fn main() -> ExFatResult<(), InMemoryBlockDevice, BLOCK_SIZE> {
    env_logger::init();
    color_backtrace::install();
    info!("reading file");

    let io = InMemoryBlockDevice::new();
    let mut fs: FileSystem<_, _, N> = FileSystem::new(io);
    let contents = fs.read_to_string("/temp2/hello2/shoe/test.txt")?;
    info!("{contents}");

    Ok(())
}

mod common;

use crate::common::blocking::InMemoryBlockDevice;
use exfat_slim::blocking::{error::ExFatError, file::OpenOptions, file_system::FileSystem};
use log::info;

// in order to use this library in a blocking way just use the `blocking` module above instead of the `asynchronous` one
fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs = FileSystem::new(io);
    let path = "hello.txt";
    fs.write(path, b"Hello, world!")?;

    let options = OpenOptions::new().write(true).append(true);
    let mut file = fs.open("hello.txt", options)?;
    file.write(&mut fs, &[b'A'; 4094])?;
    file.close(&mut fs)?;
    let text = fs.read_to_string("hello.txt")?;
    info!("{text}");

    Ok(())
}

mod common;

use crate::common::blocking::InMemoryBlockDevice;
use exfat_slim::blocking::{error::ExFatError, file::OpenOptions, file_system::FileSystem};
use log::info;
use rand::{Rng, SeedableRng, rngs::StdRng};

fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs = FileSystem::new(io);
    let path = "flipflop.txt";

    // create a new file and write "hello world" to it
    let options = OpenOptions::new().write(true).create(true).append(true);
    let mut file = fs.open(path, options)?;

    let mut dest = vec![0u8; 512];
    fill_random_ascii(&mut dest);
    let buf = dest.as_slice();

    for i in 0..2048 {
        file.write(&mut fs, buf)?;
        if i % 32 == 0 {
            info!("wrote 16KB");
        }
    }
    file.close(&mut fs)?;

    let options = OpenOptions::new().create(true).write(true).append(true);

    let _file = fs.open(path, options)?;

    /*
        let options = OpenOptions::new().read(true);
        let mut file = fs.open(path, options)?;
        let s = file.read_to_string(&mut fs)?;
        info!("{}", s.len());
    */
    Ok(())
}

fn fill_random_ascii(buf: &mut [u8]) {
    let mut rng = StdRng::seed_from_u64(42);
    for b in buf {
        *b = rng.random_range(0x20u8..=0x7Eu8);
    }
}

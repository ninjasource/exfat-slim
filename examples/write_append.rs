mod common;

use crate::common::blocking::InMemoryBlockDevice;
use exfat_slim::blocking::file::OpenOptions;
use exfat_slim::blocking::{error::ExFatError, file_system::FileSystem};
use log::info;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs = FileSystem::new(io);
    let path = "log.txt";

    let options = OpenOptions::new().write(true).create(true).append(true);
    let mut file = fs.open(path, options)?;

    let mut dest = vec![0u8; 512];
    fill_random_ascii(&mut dest);

    let count = 2 * 48 * 1000;
    for i in 0..count {
        file.write(&mut fs, &dest)?;
        if i % 32 == 0 {
            info!("wrote 16KB");
        }
    }

    let options = OpenOptions::new().write(true).append(true);
    let mut file = fs.open(path, options)?;

    let count = 2 * 48 * 1000;
    for i in 0..count {
        file.write(&mut fs, &dest)?;
        if i % 32 == 0 {
            println!("wrote 16KB");
        }
    }

    Ok(())
}

fn fill_random_ascii(buf: &mut [u8]) {
    let mut rng = StdRng::seed_from_u64(42);
    for b in buf {
        *b = rng.random_range(0x20u8..=0x7Eu8);
    }
}

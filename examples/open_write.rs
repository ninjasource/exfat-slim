mod common;
use std::str::from_utf8;

use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError> {
    env_logger::init();
    color_backtrace::install();

    let mut io = InMemoryBlockDevice::new();
    let fs = FileSystem::new(&mut io).await?;
    let full_path = "/temp2/test7.txt";

    let mut file = fs
        .with_options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&mut io, full_path)
        .await?;

    file.write(&mut io, b"hello").await?;
    file.write(&mut io, b" world").await?;
    file.seek(&mut io, 6).await?;
    file.write(&mut io, b"W").await?;

    let contents = fs.read_to_string(&mut io, full_path).await?;
    println!("Contents: `{contents}`");

    drop(file);

    let mut file = fs
        .with_options()
        .write(true)
        .append(true)
        .open(&mut io, full_path)
        .await?;

    file.write(&mut io, b". How are things?").await?;

    let contents = fs.read_to_string(&mut io, full_path).await?;
    println!("{contents}");

    let mut expected = fs.read(&mut io, full_path).await?;
    let mut dest = vec![0u8; 100000];
    fill_random_ascii(&mut dest);

    //file.write(&mut io, &dest[..100]).await?;
    file.write(&mut io, &dest).await?;

    expected.append(&mut dest);

    drop(file);

    let actual = fs.read(&mut io, full_path).await?;

    println!("expected: {:?}", from_utf8(&expected[..100]));
    println!("actual  : {:?}", from_utf8(&actual[..100]));

    for (index, (left, right)) in expected.iter().zip(&actual).enumerate() {
        if left != right {
            println!("values different at index {index}: expected {left} actual {right}");
            break;
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

/// this example tests the following scenario:
/// create a new file /temp2/test7.txt ans truncate if it already exists (it doesn't in this case)
/// make two writes (essentially writing "hello world"), change the cursor and make another write (essentially resulting in "hello World")
/// read string to confirm what was written
/// open file for write append some text resulting in the file now containing "hello World. How are things?"
/// at this point the file flags are set to NoFatChain
/// append a 100,000 random ascii characters onto the end of the file
/// this will allocate more clusters to the file and since it cannot find a contiguous set of free clusters it will remove the NoFatChain attribute indicating that this file now uses the Fat table
/// read the entire file and compare actual to expected
mod common;
use std::str::from_utf8;

use crate::common::asynchronous::InMemoryBlockDevice;
use exfat_slim::asynchronous::file::OpenBuilder;
use exfat_slim::asynchronous::{error::ExFatError, file_system::FileSystem};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ExFatError<InMemoryBlockDevice>> {
    env_logger::init();
    color_backtrace::install();

    let io = InMemoryBlockDevice::new();
    let mut fs = FileSystem::new(io).await?;
    let path = "/temp2/test7.txt";

    let options = OpenBuilder::new()
        .write(true)
        .create(true)
        .truncate(true)
        .build()?;
    let mut file = fs.open(path, options).await?;
    file.write(b"hello").await?;
    file.write(b" world").await?;
    file.seek(6).await?;
    file.write(b"W").await?;

    let contents = fs.read_to_string(path).await?;
    println!("Contents: `{contents}`");

    let options = OpenBuilder::new().write(true).append(true).build()?;
    let mut file = fs.open(path, options).await?;
    file.write(b". How are things?").await?;

    let contents = fs.read_to_string(path).await?;
    println!("{contents}");

    let mut expected = fs.read(path).await?;
    let mut dest = vec![0u8; 100000];
    fill_random_ascii(&mut dest);

    let options = OpenBuilder::new().write(true).append(true).build()?;
    let mut file = fs.open(path, options).await?;
    file.write(&dest).await?;

    expected.append(&mut dest);

    let actual = fs.read(path).await?;

    for (index, (left, right)) in expected.iter().zip(&actual).enumerate() {
        if left != right {
            println!("values different at index {index}: expected {left} actual {right}");
            let start = index;
            let end = (start + 100).min(expected.len());
            println!("expected: {:?}", from_utf8(&expected[start..end]));
            println!("actual  : {:?}", from_utf8(&actual[start..end]));
            println!("total len: {}", actual.len());

            return Ok(());
        }
    }

    println!("match success");
    Ok(())
}

fn fill_random_ascii(buf: &mut [u8]) {
    let mut rng = StdRng::seed_from_u64(42);
    for b in buf {
        *b = rng.random_range(0x20u8..=0x7Eu8);
    }
}

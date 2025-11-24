use thiserror::Error;

pub const BLOCK_SIZE: usize = 512;
pub type Block = [u8; BLOCK_SIZE];

#[derive(Error, Debug)]
pub enum Error {
    #[error("seek to position ({pos })")]
    Seek { pos: u64 },
    #[error("read exact")]
    ReadExact,
}

pub trait BlockDevice {
    fn read_sector(&mut self, sector_id: u32) -> impl Future<Output = Result<&Block, Error>>;
}

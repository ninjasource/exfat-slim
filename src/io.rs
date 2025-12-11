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
    /// reads a sector from the block device
    /// sector_id 0 must be the exfat boot sector so you may need to offset this depending on how your
    /// SD card is setup (your SD card's sector 0 will most likely point to the master boot record or GPT)
    fn read_sector(&mut self, sector_id: u32) -> impl Future<Output = Result<&Block, Error>>;

    /// writes a sector to the block device
    /// this is expected to flush
    fn write_sector(
        &mut self,
        sector_id: u32,
        block: &Block,
    ) -> impl Future<Output = Result<(), Error>>;
}

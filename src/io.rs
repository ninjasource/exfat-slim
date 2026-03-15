use core::fmt::Debug;

use super::bisync;
pub const BLOCK_SIZE: usize = 512;
pub type Block = [u8; BLOCK_SIZE];

#[allow(async_fn_in_trait)]
pub trait BlockDevice {
    type Error: Debug;

    /// reads a sector from the block device
    /// lba (logical block address - aka sector id) 0 must be the exfat boot sector so you may need to offset this depending on how your
    /// SD card is setup (your SD card's sector 0 will most likely point to the master boot record or GPT)
    #[bisync]
    async fn read(&mut self, lba: u32, block: &mut Block) -> Result<(), Self::Error>;

    /// writes an entire block to the block device
    /// you may chose to use an internal buffer here instead of flushing to disk immediatly
    /// because the user may make several writes to the same block if they are not using buffered writes
    #[bisync]
    async fn write(&mut self, lba: u32, block: &Block) -> Result<(), Self::Error>;

    /// flush internal buffers to disk media
    /// NOTE: this is optional but if it is a nop then writes above should flush to disk immediatly
    #[bisync]
    async fn flush(&mut self) -> Result<(), Self::Error>;
}

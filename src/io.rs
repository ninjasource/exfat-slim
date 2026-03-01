use core::fmt::Debug;

use super::bisync;
pub const BLOCK_SIZE: usize = 512;
pub type Block = [u8; BLOCK_SIZE];

#[allow(async_fn_in_trait)]
pub trait BlockDevice: Clone {
    type Error: Debug;

    /// reads a sector from the block device
    /// sector_id 0 must be the exfat boot sector so you may need to offset this depending on how your
    /// SD card is setup (your SD card's sector 0 will most likely point to the master boot record or GPT)
    #[bisync]
    async fn read_sector(&self, sector_id: u32, block: &mut Block) -> Result<(), Self::Error>;

    /// writes a sector to the block device
    /// this is expected to flush
    #[bisync]
    async fn write_sector(&self, sector_id: u32, block: &Block) -> Result<(), Self::Error>;
}

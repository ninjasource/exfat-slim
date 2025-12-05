use crate::io::{BLOCK_SIZE, Block, BlockDevice, Error};

pub struct InMemoryBlockDevice<'a> {
    pub sectors: &'a mut [[u8; BLOCK_SIZE]],
}

impl<'a> BlockDevice for InMemoryBlockDevice<'a> {
    async fn read_sector(&mut self, sector_id: u32) -> Result<&mut Block, Error> {
        Ok(&mut self.sectors[sector_id as usize])
    }

    async fn write_sector(&mut self, sector_id: u32, block: &Block) -> Result<(), Error> {
        self.sectors[sector_id as usize].copy_from_slice(block);
        Ok(())
    }
}

use super::{
    bisync,
    io::{BLOCK_SIZE, Block, BlockDevice, IoError},
};

#[allow(unused)]
#[derive(Debug)]
pub struct InMemoryBlockDevice<'a> {
    pub sectors: &'a mut [[u8; BLOCK_SIZE]],
}

impl<'a> BlockDevice for InMemoryBlockDevice<'a> {
    #[bisync]
    async fn read_sector(&mut self, sector_id: u32, block: &mut Block) -> Result<(), IoError> {
        block.copy_from_slice(&self.sectors[sector_id as usize]);
        Ok(())
    }

    #[bisync]
    async fn write_sector(&mut self, sector_id: u32, block: &Block) -> Result<(), IoError> {
        self.sectors[sector_id as usize].copy_from_slice(block);
        Ok(())
    }
}

// TODO: figure out how to bring in std which will make this a whole lot easier

use alloc::{sync::Arc, vec::Vec};

use super::{
    bisync,
    io::{BLOCK_SIZE, Block, BlockDevice, IoError},
};

pub struct InMemoryBlockDevice<'a> {
    inner: Arc<Mutex>,
}

#[allow(unused)]
#[derive(Debug)]
pub struct Inner<'a> {
    pub sectors: &'a mut [[u8; BLOCK_SIZE]],
    pub sectors_new: Arc<Mutex<Vec<[u8; BLOCK_SIZE]>>>,
}

impl<'a> Inner<'a> {
    #[bisync]
    async fn read_sector(&self, sector_id: u32, block: &mut Block) -> Result<(), IoError> {
        block.copy_from_slice(&self.sectors[sector_id as usize]);
        Ok(())
    }

    #[bisync]
    async fn write_sector(&self, sector_id: u32, block: &Block) -> Result<(), IoError> {
        self.sectors[sector_id as usize].copy_from_slice(block);
        Ok(())
    }
}

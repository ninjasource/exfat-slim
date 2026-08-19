use crate::blocking::BlockDevice;

use super::*;
use aligned::Aligned;
use alloc::vec::Vec;

pub(crate) const SECTOR_OFFSET: usize = 0;
pub(crate) const BLOCK_SIZE: usize = 512;

#[derive(Debug)]
pub(crate) struct DummyBlockDevice {
    pub blocks: Vec<[u8; BLOCK_SIZE]>,
}

impl DummyBlockDevice {
    pub fn new(count: usize) -> Self {
        Self {
            blocks: vec![[0u8; BLOCK_SIZE]; count],
        }
    }
}

impl BlockDevice<BLOCK_SIZE> for DummyBlockDevice {
    type Error = ();
    type Align = aligned::A4;

    fn read(
        &mut self,
        block_address: u32,
        data: &mut [Aligned<Self::Align, [u8; BLOCK_SIZE]>],
    ) -> Result<(), Self::Error> {
        data[0].copy_from_slice(&self.blocks[block_address as usize - SECTOR_OFFSET]);
        Ok(())
    }

    fn write(
        &mut self,
        block_address: u32,
        data: &[Aligned<Self::Align, [u8; BLOCK_SIZE]>],
    ) -> Result<(), Self::Error> {
        self.blocks[block_address as usize - SECTOR_OFFSET].copy_from_slice(&data[0].as_slice());
        Ok(())
    }

    fn size(&mut self) -> Result<u64, Self::Error> {
        todo!()
    }
}

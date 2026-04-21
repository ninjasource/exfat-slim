use core::array;
use core::marker::PhantomData;

use super::{
    bisync,
    error::ExFatError,
    io::{BLOCK_SIZE, Block, BlockDevice},
};

#[derive(Debug)]
pub(crate) struct Slot {
    sector_id: u32,
    is_dirty: bool, // slot must be written before eviction
    is_used: bool,  // slot was accessed recently
    is_valid: bool, // slot contains a cached sector
    block: [u8; BLOCK_SIZE],
}

impl Slot {
    pub const fn new() -> Self {
        Self {
            sector_id: 0,
            is_dirty: false,
            is_used: false,
            is_valid: false,
            block: [0u8; BLOCK_SIZE],
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8; BLOCK_SIZE] {
        self.is_dirty = true;
        &mut self.block
    }

    pub fn as_slice(&self) -> &[u8; BLOCK_SIZE] {
        &self.block
    }

    #[bisync]
    pub async fn flush<D: BlockDevice>(&mut self, io: &mut D) -> Result<(), ExFatError<D>> {
        if self.is_dirty {
            assert_ne!(self.sector_id, 0);
            io.write(self.sector_id, &self.block)
                .await
                .map_err(ExFatError::Io)?;
            self.is_dirty = false;
        }

        Ok(())
    }
}

// this cache uses the clock algorithm for least recently used slot replacement
#[derive(Debug)]
pub(crate) struct SlotCache<D: BlockDevice, const N: usize> {
    cache: [Slot; N],
    hand: usize,
    _phantom: PhantomData<D>,
}

impl<D: BlockDevice, const N: usize> SlotCache<D, N> {
    pub fn new() -> Self {
        Self {
            cache: array::from_fn(|_| Slot::new()),
            hand: 0,
            _phantom: PhantomData::default(),
        }
    }

    #[bisync]
    pub async fn flush(&mut self, io: &mut D) -> Result<(), ExFatError<D>> {
        for slot in &mut self.cache {
            slot.flush(io).await?;
        }

        Ok(())
    }

    #[bisync]
    pub async fn flush_sector(&mut self, io: &mut D, sector: u32) -> Result<(), ExFatError<D>> {
        for slot in &mut self.cache {
            if slot.sector_id == sector {
                slot.flush(io).await?;
                break;
            }
        }

        Ok(())
    }

    #[bisync]
    pub async fn read_mut(
        &mut self,
        sector_id: u32,
        io: &mut D,
    ) -> Result<&mut Slot, ExFatError<D>> {
        self.read_inner(sector_id, io).await
    }

    #[bisync]
    pub async fn read(&mut self, sector_id: u32, io: &mut D) -> Result<&Slot, ExFatError<D>> {
        let slot = self.read_inner(sector_id, io).await?;
        Ok(slot)
    }

    #[bisync]
    async fn read_inner(&mut self, sector_id: u32, io: &mut D) -> Result<&mut Slot, ExFatError<D>> {
        if let Some(index) = self.cache_hit(sector_id) {
            return Ok(&mut self.cache[index]);
        }

        let index = self.choose_victim(io).await?;
        let slot = &mut self.cache[index];
        io.read(sector_id, &mut slot.block)
            .await
            .map_err(ExFatError::Io)?;
        slot.sector_id = sector_id;
        slot.is_valid = true;
        slot.is_used = true;
        Ok(slot)
    }

    // write a full block
    #[bisync]
    pub async fn write(
        &mut self,
        io: &mut D,
        sector_id: u32,
        block: &Block,
    ) -> Result<(), ExFatError<D>> {
        if let Some(index) = self.cache_hit(sector_id) {
            // we are not interested in the bytes of this slot
            // and we don't want something to overwrite it later
            let slot = &mut self.cache[index];
            slot.block.copy_from_slice(block);
            slot.is_dirty = false;
        }

        io.write(sector_id, block).await.map_err(ExFatError::Io)?;
        Ok(())
    }

    fn cache_hit(&mut self, sector_id: u32) -> Option<usize> {
        let (left, right) = self.cache.split_at_mut(self.hand);

        for slot in right.iter_mut().chain(left) {
            if slot.sector_id == sector_id {
                slot.is_used = true;
                return Some(self.hand);
            } else {
                self.hand += 1;
                if self.hand == N {
                    self.hand = 0;
                }
            }
        }

        None
    }

    #[bisync]
    async fn choose_victim(&mut self, io: &mut D) -> Result<usize, ExFatError<D>> {
        loop {
            let i = self.hand;
            self.hand += 1;
            if self.hand == N {
                self.hand = 0;
            }

            let slot = &mut self.cache[i];

            if !slot.is_valid {
                return Ok(i);
            }

            if slot.is_used {
                // give the slot another chance
                slot.is_used = false;
                continue;
            }

            if slot.is_dirty {
                Self::write_back(io, slot).await?;
                slot.is_dirty = false;
            }

            return Ok(i);
        }
    }

    #[bisync]
    async fn write_back(io: &mut D, slot: &mut Slot) -> Result<(), ExFatError<D>> {
        io.write(slot.sector_id, &slot.block)
            .await
            .map_err(ExFatError::Io)?;
        slot.is_dirty = false;
        Ok(())
    }
}

#[allow(unused)]
#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    const SECTOR_OFFSET: usize = 100;
    #[derive(Debug)]
    struct DummyBlockDevice {
        blocks: Vec<[u8; BLOCK_SIZE]>,
    }

    impl BlockDevice for DummyBlockDevice {
        type Error = ();

        #[bisync]
        async fn read(&mut self, lba: u32, block: &mut Block) -> Result<(), Self::Error> {
            block.copy_from_slice(&self.blocks[lba as usize - SECTOR_OFFSET]);
            Ok(())
        }

        #[bisync]
        async fn write(&mut self, lba: u32, block: &Block) -> Result<(), Self::Error> {
            self.blocks[lba as usize - SECTOR_OFFSET].copy_from_slice(block);
            Ok(())
        }

        #[bisync]
        async fn flush(&mut self) -> Result<(), Self::Error> {
            todo!()
        }
    }

    use super::super::only_sync;
    use super::*;

    #[only_sync]
    #[test]
    fn read_and_write_cache() {
        let mut io = DummyBlockDevice {
            blocks: vec![
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
            ],
        };
        let mut cache = SlotCache::<DummyBlockDevice, 4>::new();

        let slot = cache.read_mut(100, &mut io).unwrap();
        slot.as_mut_slice()[..4].copy_from_slice(&[1, 2, 3, 4]);
        cache.flush(&mut io).unwrap();

        assert_eq!(&io.blocks[0][..4], &[1, 2, 3, 4]);
        for slot in &cache.cache {
            assert!(!slot.is_dirty);
        }
    }

    #[only_sync]
    #[test]
    fn evicted_cache_should_flush() {
        let mut io = DummyBlockDevice {
            blocks: vec![
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
            ],
        };
        let mut cache = SlotCache::<DummyBlockDevice, 4>::new();

        let slot0 = cache.read_mut(100, &mut io).unwrap();
        slot0.as_mut_slice()[..4].copy_from_slice(&[1, 2, 3, 4]);
        let slot1 = cache.read_mut(101, &mut io).unwrap();
        slot1.as_mut_slice()[..4].copy_from_slice(&[5, 6, 7, 8]);
        let slot2 = cache.read_mut(102, &mut io).unwrap();
        slot2.as_mut_slice()[..4].copy_from_slice(&[9, 10, 11, 12]);
        let slot3 = cache.read_mut(103, &mut io).unwrap();
        slot3.as_mut_slice()[..4].copy_from_slice(&[13, 14, 15, 16]);
        let slot4 = cache.read_mut(104, &mut io).unwrap();
        slot4.as_mut_slice()[..4].copy_from_slice(&[17, 18, 19, 20]);

        assert_eq!(&io.blocks[0][..4], &[1, 2, 3, 4]);
    }
}

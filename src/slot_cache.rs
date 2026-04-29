use core::marker::PhantomData;
use core::{array, fmt::Debug};

use aligned::Aligned;
use block_device_driver::{blocks_to_slice, blocks_to_slice_mut};

use super::{BlockDevice, bisync, error::ExFatError, file_system::ExFatResult};

#[derive(Debug)]
pub(crate) struct Slot<D, const SIZE: usize>
where
    D: BlockDevice<SIZE>,
{
    sector_id: u32,
    is_dirty: bool, // slot must be written before eviction
    is_used: bool,  // slot was accessed recently
    is_valid: bool, // slot contains a cached sector
    blocks: [Aligned<D::Align, [u8; SIZE]>; 1],
}

impl<D, const SIZE: usize> Slot<D, SIZE>
where
    D: BlockDevice<SIZE>,
{
    pub const fn new() -> Self {
        Self {
            sector_id: 0,
            is_dirty: false,
            is_used: false,
            is_valid: false,
            blocks: [Aligned([0u8; SIZE])],
        }
    }

    pub fn as_blocks_for_read(&mut self) -> &mut [Aligned<D::Align, [u8; SIZE]>] {
        &mut self.blocks
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.is_dirty = true;
        blocks_to_slice_mut(&mut self.blocks)
    }

    pub fn as_slice(&self) -> &[u8] {
        blocks_to_slice(&self.blocks)
    }

    #[bisync]
    pub async fn flush(&mut self, io: &mut D) -> ExFatResult<(), D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        if self.is_dirty {
            assert_ne!(self.sector_id, 0);
            io.write(self.sector_id, &self.blocks)
                .await
                .map_err(ExFatError::Io)?;
            self.is_dirty = false;
        }

        Ok(())
    }
}

// this cache uses the clock algorithm for least recently used slot replacement
pub(crate) struct SlotCache<D, const SIZE: usize, const N: usize>
where
    D: BlockDevice<SIZE>,
{
    cache: [Slot<D, SIZE>; N],
    hand: usize,
    _phantom: PhantomData<D>,
}

impl<D, const SIZE: usize, const N: usize> Debug for SlotCache<D, SIZE, N>
where
    D: BlockDevice<SIZE>,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlotCache")
            .field("hand", &self.hand)
            .field("_phantom", &self._phantom)
            .finish()
    }
}

impl<D, const SIZE: usize, const N: usize> SlotCache<D, SIZE, N>
where
    D: BlockDevice<SIZE>,
{
    pub fn new() -> Self {
        Self {
            cache: array::from_fn(|_| Slot::new()),
            hand: 0,
            _phantom: PhantomData::default(),
        }
    }

    #[bisync]
    pub async fn flush(&mut self, io: &mut D) -> ExFatResult<(), D, SIZE> {
        for slot in &mut self.cache {
            slot.flush(io).await?;
        }

        Ok(())
    }

    #[bisync]
    pub async fn flush_sector(&mut self, io: &mut D, sector: u32) -> ExFatResult<(), D, SIZE> {
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
    ) -> ExFatResult<&mut Slot<D, SIZE>, D, SIZE> {
        self.read_inner(sector_id, io).await
    }

    #[bisync]
    pub async fn read(
        &mut self,
        sector_id: u32,
        io: &mut D,
    ) -> ExFatResult<&Slot<D, SIZE>, D, SIZE> {
        let slot = self.read_inner(sector_id, io).await?;
        Ok(slot)
    }

    #[bisync]
    async fn read_inner(
        &mut self,
        sector_id: u32,
        io: &mut D,
    ) -> ExFatResult<&mut Slot<D, SIZE>, D, SIZE> {
        if let Some(index) = self.cache_hit(sector_id) {
            return Ok(&mut self.cache[index]);
        }

        let index = self.choose_victim(io).await?;

        let data = self.cache[index].as_blocks_for_read();
        io.read(sector_id, data).await.map_err(ExFatError::Io)?;
        let slot = &mut self.cache[index];
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
        block: &Aligned<D::Align, [u8; SIZE]>,
    ) -> ExFatResult<(), D, SIZE> {
        if let Some(index) = self.cache_hit(sector_id) {
            // we are not interested in the bytes of this slot
            // and we don't want something to overwrite it later
            let slot = &mut self.cache[index];
            slot.blocks[0].copy_from_slice(block.as_slice());
            slot.is_dirty = false;
        }

        io.write(sector_id, &[*block])
            .await
            .map_err(ExFatError::Io)?;
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
    async fn choose_victim(&mut self, io: &mut D) -> ExFatResult<usize, D, SIZE> {
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
    async fn write_back(io: &mut D, slot: &mut Slot<D, SIZE>) -> ExFatResult<(), D, SIZE> {
        io.write(slot.sector_id, &slot.blocks)
            .await
            .map_err(ExFatError::Io)?;
        slot.is_dirty = false;
        Ok(())
    }
}

#[allow(unused)]
#[cfg(test)]
mod tests {
    use super::super::only_sync;
    use super::*;
    use aligned::Aligned;
    use alloc::{vec, vec::Vec};

    const SECTOR_OFFSET: usize = 100;
    const BLOCK_SIZE: usize = 512;

    #[derive(Debug)]
    struct DummyBlockDevice {
        blocks: Vec<[u8; BLOCK_SIZE]>,
    }

    #[only_sync]
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
            self.blocks[block_address as usize - SECTOR_OFFSET]
                .copy_from_slice(&data[0].as_slice());
            Ok(())
        }

        fn size(&mut self) -> Result<u64, Self::Error> {
            todo!()
        }
    }

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
        let mut cache = SlotCache::<DummyBlockDevice, BLOCK_SIZE, 4>::new();

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
        let mut cache = SlotCache::<DummyBlockDevice, BLOCK_SIZE, 4>::new();

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

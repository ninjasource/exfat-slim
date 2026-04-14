use core::array;
use core::marker::PhantomData;

use super::{
    bisync,
    error::ExFatError,
    io::{BLOCK_SIZE, BlockDevice},
};

#[derive(Debug)]
pub struct Slot {
    sector_id: u32,
    pub is_dirty: bool, // slot must be written before eviction
    is_used: bool,      // slot was accessed recently
    is_valid: bool,     // slot contains a cached sector
    pub block: [u8; BLOCK_SIZE],
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
pub struct SlotCache<D: BlockDevice, const N: usize> {
    pub cache: [Slot; N],
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
    pub async fn read(&mut self, sector_id: u32, io: &mut D) -> Result<&mut Slot, ExFatError<D>> {
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
            }

            return Ok(i);
        }
    }

    #[bisync]
    async fn write_back(io: &mut D, slot: &mut Slot) -> Result<(), ExFatError<D>> {
        io.write(slot.sector_id, &slot.block)
            .await
            .map_err(ExFatError::Io)?;
        Ok(())
    }
}

use core::marker::PhantomData;

use super::{
    BlockDevice, bisync,
    error::ExFatError,
    file::{Touched, TouchedKind, TouchedSector},
    file_system::ExFatResult,
    slot_cache::SlotCache,
};

const MIN_CLUSER_ID: u32 = 2;
const CLUSTER_LEN: u32 = 0xFFFFFFF6;
const ENTRY_SIZE: usize = size_of::<u32>();

#[derive(Debug)]
pub struct Fat<D, const SIZE: usize, const N: usize>
where
    D: BlockDevice<SIZE>,
{
    // a sector offset from the volume boot sector
    pub start_of_fat_sector: Option<u32>,
    cache: SlotCache<D, SIZE, N>,
    _phantom: PhantomData<D>,
}

impl<D, const SIZE: usize, const N: usize> Fat<D, SIZE, N>
where
    D: BlockDevice<SIZE>,
{
    pub fn new() -> Self {
        Self {
            start_of_fat_sector: None,
            cache: SlotCache::new(),
            _phantom: PhantomData::default(),
        }
    }

    #[bisync]
    pub async fn flush(&mut self, io: &mut D) -> ExFatResult<(), D, SIZE> {
        self.cache.flush(io).await?;
        Ok(())
    }

    #[bisync]
    pub async fn flush_sector(&mut self, io: &mut D, sector: u32) -> ExFatResult<(), D, SIZE> {
        self.cache.flush_sector(io, sector).await?;
        Ok(())
    }

    /// sets a fat record to build up the fat chain
    /// a cluster_id_to of 0 is used for unlinking
    #[bisync]
    pub(crate) async fn set(
        &mut self,
        io: &mut D,
        touched: &mut impl Touched,
        cluster_id: u32,
        cluster_id_to: u32,
    ) -> ExFatResult<(), D, SIZE> {
        assert!(cluster_id >= MIN_CLUSER_ID);
        let sector_id = self.get_sector_id(cluster_id)?;
        touched.insert(TouchedSector::new(TouchedKind::Fat, sector_id));
        let slot = self.cache.read_mut(sector_id, io).await?;

        let (chunks, _remainder) = slot.as_mut_slice().as_chunks_mut::<ENTRY_SIZE>();
        let sector_offset = (cluster_id % Self::num_entries()) as usize;
        chunks[sector_offset].copy_from_slice(&cluster_id_to.to_le_bytes());

        Ok(())
    }

    const fn num_entries() -> u32 {
        (SIZE / ENTRY_SIZE) as u32
    }

    fn get_sector_id(&self, cluster_id: u32) -> ExFatResult<u32, D, SIZE> {
        match self.start_of_fat_sector {
            Some(fat_offset) => Ok(fat_offset + cluster_id / Self::num_entries()),
            None => Err(ExFatError::Unexpected(
                "attemt to access fat when not initialized",
            )),
        }
    }

    /// gets the next cluster_id in the fat chain
    #[bisync]
    pub(crate) async fn next_cluster_in_fat_chain(
        &mut self,
        cluster_id: u32,
        io: &mut D,
    ) -> ExFatResult<Option<u32>, D, SIZE> {
        assert!(cluster_id >= MIN_CLUSER_ID);
        let sector_id = self.get_sector_id(cluster_id)?;
        let sector_offset = (cluster_id % Self::num_entries()) as usize;

        let slot = self.cache.read(sector_id, io).await?;
        let (chunks, _remainder) = slot.as_slice().as_chunks::<ENTRY_SIZE>();
        let next_cluster_id = u32::from_le_bytes(chunks[sector_offset]);

        if (MIN_CLUSER_ID..CLUSTER_LEN).contains(&next_cluster_id) {
            Ok(Some(next_cluster_id))
        } else {
            Ok(None)
        }
    }
}

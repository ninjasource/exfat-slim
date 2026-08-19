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
            _phantom: PhantomData,
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

        if (MIN_CLUSER_ID..=CLUSTER_LEN).contains(&next_cluster_id) {
            Ok(Some(next_cluster_id))
        } else {
            Ok(None)
        }
    }
}

#[allow(unused)]
#[cfg(test)]
mod tests {
    use super::super::only_sync;
    use super::*;
    const END_OF_CHAIN: u32 = 0xFFFF_FFFF;

    use crate::{
        blocking::file::{FileDirty, NO_CLUSTER_ID},
        test_utils::{BLOCK_SIZE, DummyBlockDevice},
    };

    #[only_sync]
    #[test]
    fn fat_chain_crosses_fat_sector_boundary() {
        // arrange - a 512 byte sector holds 128 u23 entries to cluster 128 is
        // the first entry of the second fat sector
        let mut io = DummyBlockDevice::new(4);
        let mut fat = Fat::<DummyBlockDevice, BLOCK_SIZE, 4>::new();
        fat.start_of_fat_sector = Some(0);
        let mut touched = FileDirty::new();

        // act - link 126 -> 127 -> 128 -> 129 -> end of chain
        fat.set(&mut io, &mut touched, 126, 127).unwrap();
        fat.set(&mut io, &mut touched, 127, 128).unwrap();
        fat.set(&mut io, &mut touched, 128, 129).unwrap();
        fat.set(&mut io, &mut touched, 129, END_OF_CHAIN).unwrap();

        // confirm that nothing reaches the device until a flush
        assert_eq!(io.blocks[0][504..512], [0u8; 8]);
        fat.flush(&mut io).unwrap();

        // entries in the correct place in the correct sector
        assert_eq!(&io.blocks[0][504..508], &127u32.to_le_bytes()); // cluster 126
        assert_eq!(&io.blocks[0][508..512], &128u32.to_le_bytes()); // cluster 127
        assert_eq!(&io.blocks[1][0..4], &129u32.to_le_bytes()); // cluster 128
        assert_eq!(&io.blocks[1][4..8], &END_OF_CHAIN.to_le_bytes()); // cluster 129

        // walk across sector boundary works as expected
        assert_eq!(
            fat.next_cluster_in_fat_chain(126, &mut io).unwrap(),
            Some(127)
        );
        assert_eq!(
            fat.next_cluster_in_fat_chain(127, &mut io).unwrap(),
            Some(128)
        );
        assert_eq!(
            fat.next_cluster_in_fat_chain(128, &mut io).unwrap(),
            Some(129)
        );
        assert_eq!(fat.next_cluster_in_fat_chain(129, &mut io).unwrap(), None);
    }

    #[only_sync]
    #[test]
    fn unlinking_cluster_breaks_chain() {
        let mut io = DummyBlockDevice::new(2);

        let mut fat = Fat::<DummyBlockDevice, BLOCK_SIZE, 4>::new();
        fat.start_of_fat_sector = Some(0);
        let mut touched = FileDirty::new();

        fat.set(&mut io, &mut touched, 2, 3).unwrap();
        fat.set(&mut io, &mut touched, 3, END_OF_CHAIN).unwrap();
        assert_eq!(fat.next_cluster_in_fat_chain(2, &mut io).unwrap(), Some(3));
        assert_eq!(fat.next_cluster_in_fat_chain(3, &mut io).unwrap(), None);

        // act
        fat.set(&mut io, &mut touched, 2, NO_CLUSTER_ID).unwrap();

        // assert
        assert_eq!(fat.next_cluster_in_fat_chain(2, &mut io).unwrap(), None);
        io.blocks[0].fill(0xAA); // just so that the very last assert is actually tested
        fat.flush(&mut io).unwrap();
        assert_eq!(&io.blocks[0][8..12], &[0, 0, 0, 0]);
    }

    #[only_sync]
    #[test]
    fn fat_entry_values_that_terminate_a_chain() {
        let mut io = DummyBlockDevice::new(2);
        let mut fat = Fat::<DummyBlockDevice, BLOCK_SIZE, 4>::new();
        fat.start_of_fat_sector = Some(0);
        let mut touched = FileDirty::new();

        let cases = [
            (0x0000_0000, None),
            (0x0000_0001, None),
            (0x0000_0002, Some(0x0000_0002)),
            (0xFFFF_FFF5, Some(0xFFFF_FFF5)),
            (0xFFFF_FFF6, Some(0xFFFF_FFF6)),
            (0xFFFF_FFF7, None),
            (0xFFFF_FFFF, None),
        ];

        for (entry, expected) in cases {
            fat.set(&mut io, &mut touched, 2, entry).unwrap();
            assert_eq!(
                fat.next_cluster_in_fat_chain(2, &mut io).unwrap(),
                expected,
                "fat entry {entry:#010x}"
            )
        }
    }
}

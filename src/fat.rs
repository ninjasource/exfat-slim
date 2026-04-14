use core::marker::PhantomData;

use super::{
    bisync,
    error::ExFatError,
    io::{BLOCK_SIZE, BlockDevice},
    slot_cache::SlotCache,
};

const MIN_CLUSER_ID: u32 = 2;
const CLUSTER_LEN: u32 = 0xFFFFFFF6;
const ENTRY_SIZE: usize = size_of::<u32>();
const NUM_ENTRIES: usize = BLOCK_SIZE / ENTRY_SIZE;

#[derive(Debug)]
pub struct Fat<D: BlockDevice> {
    pub fat_offset: Option<u32>,
    cache: SlotCache<D, 4>,
    _phantom: PhantomData<D>,
}

impl<D: BlockDevice> Fat<D> {
    pub fn new() -> Self {
        Self {
            fat_offset: None,
            cache: SlotCache::new(),
            _phantom: PhantomData::default(),
        }
    }

    #[bisync]
    pub async fn flush(&mut self, io: &mut D) -> Result<(), ExFatError<D>> {
        self.cache.flush(io).await?;
        Ok(())
    }

    /// sets a fat record to build up the fat chain
    /// a cluster_id_to of 0 is used for unlinking
    #[bisync]
    pub(crate) async fn set(
        &mut self,
        cluster_id: u32,
        cluster_id_to: u32,
        io: &mut D,
    ) -> Result<(), ExFatError<D>> {
        let sector_id = self.get_sector_id(cluster_id)?;
        let slot = self.cache.read(sector_id, io).await?;

        let (chunks, _remainder) = slot.block.as_chunks_mut::<ENTRY_SIZE>();
        let sector_offset = (cluster_id % NUM_ENTRIES as u32) as usize;
        chunks[sector_offset].copy_from_slice(&cluster_id_to.to_le_bytes());
        slot.is_dirty = true;

        Ok(())
    }

    fn get_sector_id(&self, cluster_id: u32) -> Result<u32, ExFatError<D>> {
        match self.fat_offset {
            Some(fat_offset) => Ok(fat_offset + cluster_id / NUM_ENTRIES as u32),
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
    ) -> Result<Option<u32>, ExFatError<D>> {
        let sector_id = self.get_sector_id(cluster_id)?;
        let sector_offset = (cluster_id % NUM_ENTRIES as u32) as usize;

        let slot = self.cache.read(sector_id, io).await?;
        let (chunks, _remainder) = slot.block.as_chunks::<ENTRY_SIZE>();
        let next_cluster_id = u32::from_le_bytes(chunks[sector_offset]);

        if (MIN_CLUSER_ID..CLUSTER_LEN).contains(&next_cluster_id) {
            Ok(Some(next_cluster_id))
        } else {
            Ok(None)
        }
    }
}

/*
/// TODO: REMOVE THIS!!!
/// gets the next cluster_id in the fat chain
#[bisync]
pub(crate) async fn next_cluster_in_fat_chain<D: BlockDevice>(
    io: &mut D,
    fat_offset: u32, // from boot_sector
    cluster_id: u32,
) -> Result<Option<u32>, ExFatError<D>> {
    let sector_id = fat_offset + cluster_id / NUM_ENTRIES as u32;
    let sector_offset = (cluster_id % NUM_ENTRIES as u32) as usize;

    let mut block = [0u8; BLOCK_SIZE];
    io.read(sector_id, &mut block)
        .await
        .map_err(ExFatError::Io)?;
    let (chunks, _remainder) = block.as_chunks::<ENTRY_SIZE>();
    let next_cluster_id = u32::from_le_bytes(chunks[sector_offset]);

    if (MIN_CLUSER_ID..CLUSTER_LEN).contains(&next_cluster_id) {
        Ok(Some(next_cluster_id))
    } else {
        Ok(None)
    }
}
*/
/*
/// TODO: REMOVE THIS!!!
/// updates the fat chain
#[bisync]
pub(crate) async fn update_fat_chain<D: BlockDevice>(
    io: &mut D,
    fat_offset: u32, // from boot_sector
    cluster_ids: &[u32],
) -> Result<(), ExFatError<D>> {
    // the next cluster that each cluster points to (cluster_id, next_cluster_id)
    // e.g. &[1,2,3,4] => [(1,2), (2,3), (3,4)]
    let cluster_id_pairs = cluster_ids.windows(2).map(|w| (w[0], w[1]));

    // bucket cluster fat mapping by sector_id so that we can write an entire sector at a time
    let mut by_sector_id = BTreeMap::new();

    // build a dictionary cluster mappings keyed by sector_id
    for (cluster_id, next_cluster_id) in cluster_id_pairs {
        let sector_id = fat_offset + cluster_id / NUM_ENTRIES as u32;
        let value = (cluster_id, next_cluster_id);
        by_sector_id
            .entry(sector_id)
            .or_insert_with(Vec::new)
            .push(value);
    }

    let mut block = [0u8; BLOCK_SIZE];
    for (sector_id, value) in by_sector_id {
        // read entire sector
        io.read(sector_id, &mut block)
            .await
            .map_err(ExFatError::Io)?;

        let (chunks, _remainder) = block.as_chunks_mut::<ENTRY_SIZE>();

        // mutate only the chunks pertaining to our cluster_ids
        for (cluster_id, next_cluster_id) in value {
            let sector_offset = (cluster_id % NUM_ENTRIES as u32) as usize;
            chunks[sector_offset].copy_from_slice(&next_cluster_id.to_le_bytes());
        }

        // write entire sector
        io.write(sector_id, &block).await.map_err(ExFatError::Io)?;
    }

    Ok(())
}*/

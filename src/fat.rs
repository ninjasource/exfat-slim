use crate::{
    error::ExFatError,
    io::{BLOCK_SIZE, BlockDevice},
};

/// gets the next cluster_id in the fat chain
pub async fn next_cluster_in_fat_chain(
    io: &mut impl BlockDevice,
    fat_offset: u32, // from boot_sector
    cluster_id: u32,
) -> Result<Option<u32>, ExFatError> {
    const ENTRY_SIZE: usize = size_of::<u32>();
    const NUM_ENTRIES: usize = BLOCK_SIZE / ENTRY_SIZE;
    let sector_id = fat_offset + cluster_id / NUM_ENTRIES as u32;
    let sector_offset = (cluster_id % NUM_ENTRIES as u32) as usize;

    let buf = io.read_sector(sector_id).await?;
    let (chunks, _remainder) = buf.as_chunks::<ENTRY_SIZE>();
    let next_cluster_id = u32::from_le_bytes(chunks[sector_offset]);

    if (2..0xFFFFFFF6).contains(&next_cluster_id) {
        Ok(Some(next_cluster_id))
    } else {
        Ok(None)
    }
}

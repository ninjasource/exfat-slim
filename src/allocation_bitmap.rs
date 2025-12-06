use core::num;

use alloc::{collections::btree_map::BTreeMap, vec::Vec};

use crate::{
    directory_entry::AllocationBitmapDirEntry,
    error::ExFatError,
    file_system::FileSystemDetails,
    io::{BLOCK_SIZE, Block, BlockDevice},
};

#[derive(Debug)]
pub struct AllocationBitmap {
    /// start of the allocation bitmap table
    pub first_cluster: u32,
    /// size, in sectors, of the allocation bitmap
    num_sectors: u32,
    /// max cluster_id in the bitmap table
    max_cluster_id: u32,
}

pub enum Allocation {
    Contiguous { first_cluster: u32 },
    FatChain { first_cluster: u32 },
}

impl AllocationBitmap {
    pub fn new(alloc_bitmap: &AllocationBitmapDirEntry) -> Self {
        let extra_sector = if alloc_bitmap.data_length as usize % BLOCK_SIZE > 0 {
            1
        } else {
            0
        };
        let num_sectors = (alloc_bitmap.data_length as u32 / BLOCK_SIZE as u32) + extra_sector;
        // the allocation bitmap starts at cluster id 2
        let max_cluster_id = alloc_bitmap.data_length as u32 * size_of::<u8>() as u32 + 2;

        Self {
            first_cluster: alloc_bitmap.first_cluster,
            num_sectors,
            max_cluster_id,
        }
    }

    pub async fn mark_allocated(
        &self,
        io: &mut impl BlockDevice,
        fs: &FileSystemDetails,
        cluster_ids: &[u32],
        allocated: bool,
    ) -> Result<(), ExFatError> {
        let mut by_sector_id = BTreeMap::<u32, Vec<usize>>::new();
        for cluster_id in cluster_ids {
            let first_sector_id = fs.get_heap_sector_id(self.first_cluster)?;
            let sector_id = first_sector_id + (cluster_id - 2) / BLOCK_SIZE as u32;
            let index = (*cluster_id as usize - 2) % BLOCK_SIZE;
            by_sector_id
                .entry(sector_id)
                .or_insert(Vec::new())
                .push(index);
        }

        self.mark_allocated_by_sector(io, fs, &by_sector_id, allocated)
            .await?;
        Ok(())
    }

    pub async fn mark_allocated_contiguous(
        &self,
        io: &mut impl BlockDevice,
        fs: &FileSystemDetails,
        first_cluster: u32,
        num_clusters: u32,
        allocated: bool,
    ) -> Result<(), ExFatError> {
        let mut by_sector_id = BTreeMap::<u32, Vec<usize>>::new();
        for cluster_id in first_cluster..first_cluster + num_clusters {
            let first_sector_id = fs.get_heap_sector_id(self.first_cluster)?;
            let sector_id = first_sector_id + (cluster_id - 2) / BLOCK_SIZE as u32;
            let index = (cluster_id as usize - 2) % BLOCK_SIZE;
            by_sector_id
                .entry(sector_id)
                .or_insert(Vec::new())
                .push(index);
        }

        self.mark_allocated_by_sector(io, fs, &by_sector_id, allocated)
            .await?;
        Ok(())
    }

    pub async fn mark_allocated_by_sector(
        &self,
        io: &mut impl BlockDevice,
        fs: &FileSystemDetails,
        by_sector_id: &BTreeMap<u32, Vec<usize>>,
        allocated: bool,
    ) -> Result<(), ExFatError> {
        for (sector_id, indices) in by_sector_id {
            let mut block = [0u8; BLOCK_SIZE];
            block.copy_from_slice(io.read_sector(*sector_id).await?);
            for index in indices {
                let byte = &mut block[index / 8];
                let bit_offset = 8 - index % 8; // count bits from right to left
                if allocated {
                    // set the bit
                    *byte |= 1 << bit_offset;
                } else {
                    // unset the bit
                    *byte &= !(1 << bit_offset);
                }
            }
            io.write_sector(*sector_id, &block).await?;
        }

        Ok(())
    }

    /// locates the next free set of contiguous clusters
    pub async fn find_free_clusters(
        &self,
        io: &mut impl BlockDevice,
        fs: &FileSystemDetails,
        num_clusters: u32,
        only_fat_chain: bool,
    ) -> Result<Allocation, ExFatError> {
        let sector_id = fs.get_heap_sector_id(self.first_cluster)?;

        let mut counter = 0;
        let mut cluster_id: Option<u32> = None;
        let mut first_free: Option<u32> = None;

        // let num_sectors
        'outer: for sector_offset in 0..self.num_sectors {
            let buf = io.read_sector(sector_id + sector_offset).await?;
            let (bytes, _remainder) = buf.as_chunks::<4>();
            for (index, bytes) in bytes.iter().enumerate() {
                let val = u32::from_be_bytes(*bytes);
                // fast batch check for a free cluster
                if val != u32::MAX {
                    // locate the first free cluster
                    for bit in (0..u32::BITS).rev() {
                        if val & (1 << bit) == 0 {
                            // happy path
                            let cluster_id = match cluster_id {
                                Some(cluster_id) => {
                                    counter += 1;
                                    cluster_id
                                }
                                None => {
                                    let cluster_id = sector_offset as u32 * BLOCK_SIZE as u32
                                        + index as u32 * bit as u32
                                        + 2;

                                    if first_free.is_none() {
                                        // keep track of the first free cluster we ever found
                                        first_free = Some(cluster_id);
                                        if only_fat_chain {
                                            break 'outer;
                                        }
                                    }
                                    counter = 1;
                                    cluster_id
                                }
                            };

                            // abandon attepting to
                            if cluster_id > self.max_cluster_id {
                                break;
                            }

                            if counter == num_clusters {
                                return Ok(Allocation::Contiguous {
                                    first_cluster: cluster_id,
                                });
                            }
                        } else {
                            if cluster_id.is_some() {
                                cluster_id = None;
                                counter = 0;
                            }
                        }
                    }
                }
            }
        }

        // we could not find a contiguous block of clusters
        return match first_free {
            Some(first_free) if first_free <= self.max_cluster_id => Ok(Allocation::FatChain {
                first_cluster: first_free,
            }),
            _ => Err(ExFatError::DiskFull),
        };
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::directory_entry::BitmapFlags;
    use crate::mocks::InMemoryBlockDevice;

    #[tokio::test]
    async fn find_free_clusteres_test() {
        let mut sectors = [[0; BLOCK_SIZE], [0; BLOCK_SIZE]];
        let mut io = InMemoryBlockDevice {
            sectors: &mut sectors,
        };
        let dir_entry = AllocationBitmapDirEntry {
            bitmap_flags: BitmapFlags::FirstOrSecondBitmap,
            first_cluster: 2,
            data_length: BLOCK_SIZE as u64,
        };
        let allocation_bitmap = AllocationBitmap::new(&dir_entry);
        let fs = FileSystemDetails {
            cluster_heap_offset: 0,
            fat_offset: 123,
            sectors_per_cluster: 2,
            cluster_length: 10,
            first_cluster_of_root_dir: 0,
        };
        allocation_bitmap
            .find_free_clusters(&mut io, &fs, 1, false)
            .await
            .unwrap();
        //fut.poll();
    }
}

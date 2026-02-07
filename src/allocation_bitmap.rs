/// the allocation bitmap signals whether or not a cluster is in use.
/// the AllocationBitmapDirEntry identifies where to locate the bitmap.
/// for example, first_cluster 2 means that the allocation bitmap is the very first cluster in the cluster heap (cluster id 0 and 1 are not valid)
/// each bit in the allocation bitmap points to a cluster (starting at cluster 2).
/// therefore the following bits (as they are layed out in memory) map to the following clusters
///         [byte 0                ][byte 1                ][byte 2                ][byte 3                ]
/// bit      7  6  5  4  3  2  1  0  7  6  5  4  3  2  1  0  7  6  5  4  3  2  1  0  7  6  5  4  3  2  1  0
/// cluster  9  8  7  6  5  4  3  2 17 16 15 14 13 12 11 10 25 24 23 22 21 20 19 18 33 32 31 30 29 28 27 26
/// NOTE: the above layout is not obvious from the spec but it has been confirmed with how the windows implementation writes the bits.
/// for example an allocation bitmap of this bit string "11111111 11111111 00000111" means that clusters 2 - 20 inclusive are allocated. That is 19 clusters in total.
///
/// NOTE: I encountered what appears to be a logic error in the linux kernel exfat implementation where bits are incorrectly counted from MSB to LSB and not the other way around when locating free allocations.
/// this does not affect the allocation bitmap write consistency but could possibly lead to unnecessary fragmentation
///
use core::num;

use alloc::{collections::btree_map::BTreeMap, vec, vec::Vec};

use super::{
    bisync,
    directory_entry::AllocationBitmapDirEntry,
    error::ExFatError,
    file_system::FileSystemDetails,
    io::{BLOCK_SIZE, Block, BlockDevice},
    only_async,
};

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, Clone)]
pub struct AllocationBitmap {
    /// start of the allocation bitmap table
    pub first_cluster: u32,
    /// size, in sectors, of the allocation bitmap
    pub num_sectors: u32,
    /// max cluster_id in the bitmap table
    pub max_cluster_id: u32,
}

pub enum Allocation {
    Contiguous {
        first_cluster: u32,
        num_clusters: u32,
    },
    FatChain {
        clusters: Vec<u32>,
    },
}

impl AllocationBitmap {
    pub fn new(alloc_bitmap: &AllocationBitmapDirEntry) -> Self {
        let num_sectors = alloc_bitmap.data_length.div_ceil(BLOCK_SIZE as u64) as u32;

        // the allocation bitmap starts at cluster id 2
        let max_cluster_id = alloc_bitmap.data_length as u32 * size_of::<u8>() as u32 + 2;

        Self {
            first_cluster: alloc_bitmap.first_cluster,
            num_sectors,
            max_cluster_id,
        }
    }

    fn calc_allocation_position(
        first_sector_id: u32,
        cluster_id: u32,
    ) -> Result<(u32, usize), ExFatError> {
        if cluster_id < 2 {
            return Err(ExFatError::InvalidClusterId(cluster_id));
        }

        let sector_id = first_sector_id + (cluster_id - 2) / BLOCK_SIZE as u32;
        let index = (cluster_id as usize - 2) % BLOCK_SIZE;
        Ok((sector_id, index))
    }

    #[bisync]
    pub async fn mark_allocated(
        &self,
        io: &mut impl BlockDevice,
        fs: &FileSystemDetails,
        cluster_ids: &[u32],
        allocated: bool,
    ) -> Result<(), ExFatError> {
        let mut by_sector_id = BTreeMap::<u32, Vec<usize>>::new();
        let first_sector_id = fs.get_heap_sector_id(self.first_cluster)?;

        for cluster_id in cluster_ids {
            let (sector_id, index) = Self::calc_allocation_position(first_sector_id, *cluster_id)?;
            by_sector_id
                .entry(sector_id)
                .or_insert(Vec::new())
                .push(index);
        }

        self.mark_allocated_by_sector(io, fs, &by_sector_id, allocated)
            .await?;
        Ok(())
    }

    #[bisync]
    pub async fn mark_allocated_contiguous(
        &self,
        io: &mut impl BlockDevice,
        fs: &FileSystemDetails,
        first_cluster: u32,
        num_clusters: u32,
        allocated: bool,
    ) -> Result<(), ExFatError> {
        let mut by_sector_id = BTreeMap::<u32, Vec<usize>>::new();
        let first_sector_id = fs.get_heap_sector_id(self.first_cluster)?;

        for cluster_id in first_cluster..first_cluster + num_clusters {
            let (sector_id, index) = Self::calc_allocation_position(first_sector_id, cluster_id)?;
            by_sector_id
                .entry(sector_id)
                .or_insert(Vec::new())
                .push(index);
        }

        self.mark_allocated_by_sector(io, fs, &by_sector_id, allocated)
            .await?;
        Ok(())
    }

    #[bisync]
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
                let bit = index % 8;
                let is_set = (*byte & (1 << bit)) != 0;

                // an attempt to change the allocation that will have no effect
                if allocated && is_set || !allocated && !is_set {
                    return Err(ExFatError::InvalidAllocation);
                }

                if allocated {
                    // set the bit
                    *byte |= 1 << bit;
                } else {
                    // unset the bit
                    *byte &= !(1 << bit);
                }
            }

            io.write_sector(*sector_id, &block).await?;
        }

        Ok(())
    }

    /// locates the next free set of contiguous clusters.
    /// if from_cluster is Some it will, like all clusters, only be included if it is NOT allocated.
    #[bisync]
    pub async fn find_free_clusters_non_contiguous(
        &self,
        io: &mut impl BlockDevice,
        fs: &FileSystemDetails,
        num_clusters: usize,
        from_cluster: Option<u32>,
    ) -> Result<Allocation, ExFatError> {
        let from_cluster = from_cluster.unwrap_or(self.first_cluster);
        let sector_id = fs.get_heap_sector_id(from_cluster)?;
        let sector_index = (from_cluster - 2) / BLOCK_SIZE as u32;
        let mut cluster_id = sector_index * BLOCK_SIZE as u32 + 2;
        let mut clusters = Vec::with_capacity(num_clusters);

        for sector_offset in sector_index..self.num_sectors {
            let buf = io.read_sector(sector_id + sector_offset).await?;
            let (bytes, _remainder) = buf.as_chunks::<4>();
            for (chunk_index, chunk) in bytes.iter().enumerate() {
                if u32::from_le_bytes(*chunk) != u32::MAX {
                    for (byte_index, byte) in chunk.iter().enumerate() {
                        for bit in 0..u8::BITS {
                            if *byte & 1 << bit == 0 {
                                if from_cluster <= cluster_id {
                                    clusters.push(cluster_id);
                                    if clusters.len() == num_clusters {
                                        return Ok(Allocation::FatChain { clusters });
                                    }
                                }
                            }

                            cluster_id += 1;
                        }
                    }
                } else {
                    cluster_id += u32::BITS;
                }
            }
        }

        Err(ExFatError::DiskFull)
    }

    /// locates the next free set of contiguous clusters.
    /// if from_cluster is Some it will, like all clusters, only be included if it is NOT allocated.
    #[bisync]
    pub async fn find_free_clusters_contiguous(
        &self,
        io: &mut impl BlockDevice,
        fs: &FileSystemDetails,
        num_clusters: usize,
        from_cluster: Option<u32>,
    ) -> Result<Allocation, ExFatError> {
        let from_cluster = from_cluster.unwrap_or(self.first_cluster);
        let sector_id = fs.get_heap_sector_id(from_cluster)?;
        let sector_index = (from_cluster - 2) / BLOCK_SIZE as u32;
        let mut cluster_id = sector_index * BLOCK_SIZE as u32 + 2;
        let mut first_cluster = None;
        let mut count = 0;

        for sector_offset in sector_index..self.num_sectors {
            let buf = io.read_sector(sector_id + sector_offset).await?;
            let (bytes, _remainder) = buf.as_chunks::<4>();
            for (chunk_index, chunk) in bytes.iter().enumerate() {
                if u32::from_le_bytes(*chunk) != u32::MAX {
                    for (byte_index, byte) in chunk.iter().enumerate() {
                        for bit in 0..u8::BITS {
                            if *byte & 1 << bit == 0 {
                                if from_cluster <= cluster_id {
                                    if first_cluster.is_none() {
                                        first_cluster = Some(cluster_id);
                                        count = 1
                                    } else {
                                        count += 1;
                                    }

                                    if count == num_clusters {
                                        return Ok(Allocation::Contiguous {
                                            first_cluster: first_cluster.unwrap(),
                                            num_clusters: num_clusters as u32,
                                        });
                                    }
                                }
                            } else {
                                first_cluster = None;
                            }

                            cluster_id += 1;
                        }
                    }
                } else {
                    cluster_id += u32::BITS;
                    first_cluster = None;
                }
            }
        }

        Err(ExFatError::DiskFull)
    }

    // TODO: combine calls to continuous and non_continuous in this function (replace its contents)
    #[bisync]
    pub async fn find_free_clusters(
        &self,
        io: &mut impl BlockDevice,
        fs: &FileSystemDetails,
        num_clusters: u32,
        only_fat_chain: bool,
        from_cluster: Option<u32>,
    ) -> Result<Allocation, ExFatError> {
        let from_cluster = from_cluster.unwrap_or(self.first_cluster);
        let sector_id = fs.get_heap_sector_id(from_cluster)?;

        let mut counter = 0;
        let mut cluster_id: Option<u32> = None;
        let mut first_free: Option<u32> = None;

        // let num_sectors
        'outer: for sector_offset in 0..self.num_sectors {
            let buf = io.read_sector(sector_id + sector_offset).await?;
            let (bytes, _remainder) = buf.as_chunks::<4>();
            for (chunk_index, chunk) in bytes.iter().enumerate() {
                // fast batch check for a free cluster
                if u32::from_le_bytes(*chunk) != u32::MAX {
                    for (byte_index, byte) in chunk.iter().enumerate() {
                        for bit in 0..u8::BITS {
                            if *byte & 1 << bit == 0 {
                                // happy path
                                let cluster_id = match cluster_id {
                                    Some(cluster_id) => {
                                        counter += 1;
                                        cluster_id
                                    }
                                    None => {
                                        // position 0 cluster_id starts at 2 (for historical reasons they say)
                                        let cluster_id = sector_offset as u32 * BLOCK_SIZE as u32
                                            + chunk_index as u32 * u32::BITS
                                            + byte_index as u32 * u8::BITS
                                            + bit as u32
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

                                // abandon attepting to find an allocation
                                if cluster_id > self.max_cluster_id {
                                    break;
                                }

                                if counter == num_clusters {
                                    return Ok(Allocation::Contiguous {
                                        first_cluster: cluster_id,
                                        num_clusters,
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
        }

        // we could not find a contiguous block of clusters
        return match first_free {
            Some(first_free) if first_free <= self.max_cluster_id => Ok(Allocation::FatChain {
                clusters: vec![first_free],
            }),
            _ => Err(ExFatError::DiskFull),
        };
    }
}

#[only_async]
#[cfg(test)]
mod tests {

    use super::super::{directory_entry::BitmapFlags, mocks::InMemoryBlockDevice};
    use super::*;

    #[tokio::test]
    async fn find_free_clusters() {
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
            .find_free_clusters(&mut io, &fs, 1, false, None)
            .await
            .unwrap();
    }
}

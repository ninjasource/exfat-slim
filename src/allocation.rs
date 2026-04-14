use core::marker::PhantomData;

use super::{
    bisync,
    error::ExFatError,
    fat::Fat,
    file::NO_CLUSTER_ID,
    io::{BLOCK_SIZE, BlockDevice},
    slot_cache::SlotCache,
};

const FIRST_CLUSTER_ID: u32 = 2;

pub struct AllocatedRun {
    pub first_cluster: u32,
    pub cluster_count: u32,
}

#[derive(Debug, Clone)]
pub enum StoredChain {
    Empty,
    Contiguous {
        first: u32,
        cluster_count: u32,
    },
    Fat {
        first: u32,
        last: u32,
        cluster_count: u32,
    },
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, Default)]
pub struct AllocationBitmapSlim {
    pub first_sector: u32,
    pub num_sectors: u32,
}

#[derive(Debug)]
pub struct Allocator<D: BlockDevice> {
    pub bitmap: AllocationBitmapSlim,
    cache: SlotCache<D, 4>,
    next_search_cluster: u32,
    _phantom: PhantomData<D>,
}

impl<D: BlockDevice> Allocator<D> {
    pub fn new() -> Self {
        Self {
            bitmap: AllocationBitmapSlim::default(),
            cache: SlotCache::new(),
            next_search_cluster: FIRST_CLUSTER_ID,
            _phantom: PhantomData::default(),
        }
    }

    #[bisync]
    pub async fn flush(&mut self, io: &mut D) -> Result<(), ExFatError<D>> {
        self.cache.flush(io).await?;
        Ok(())
    }

    #[bisync]
    pub async fn allocate(
        &mut self,
        io: &mut D,
        chain: &StoredChain,
        count: u32,
    ) -> Result<AllocatedRun, ExFatError<D>> {
        match chain {
            StoredChain::Empty => {
                let run = self.find_free_clusters(io, count).await?;
                self.mark_allocated(io, &run, true).await?;
                Ok(run)
            }
            StoredChain::Contiguous {
                first,
                cluster_count,
            } => {
                let from_cluster = first + cluster_count;
                let run = self
                    .find_free_clusters_from(io, from_cluster, count)
                    .await?;
                self.mark_allocated(io, &run, true).await?;
                Ok(run)
            }
            StoredChain::Fat {
                first: _first,
                last,
                cluster_count: _len,
            } => {
                let from_cluster = last + 1;
                let run = self
                    .find_free_clusters_from(io, from_cluster, count)
                    .await?;
                self.mark_allocated(io, &run, true).await?;
                Ok(run)
            }
        }
    }

    #[bisync]
    pub async fn free(
        &mut self,
        io: &mut D,
        chain: &StoredChain,
        fat: &mut Fat<D>,
    ) -> Result<(), ExFatError<D>> {
        match chain {
            StoredChain::Empty => {
                // nothing to do
            }
            StoredChain::Contiguous {
                first,
                cluster_count,
            } => {
                let run = AllocatedRun {
                    first_cluster: *first,
                    cluster_count: *cluster_count,
                };
                self.mark_allocated(io, &run, false).await?
            }
            StoredChain::Fat {
                first,
                last: _last,
                cluster_count: _cluster_count,
            } => {
                let mut cluster_id = *first;

                while let Some(next_cluster_id) =
                    fat.next_cluster_in_fat_chain(cluster_id, io).await?
                {
                    let run = AllocatedRun {
                        first_cluster: cluster_id,
                        cluster_count: 1,
                    };
                    self.mark_allocated(io, &run, false).await?;
                    fat.set(cluster_id, NO_CLUSTER_ID, io).await?;
                    cluster_id = next_cluster_id;
                }

                // the last cluster
                fat.set(cluster_id, NO_CLUSTER_ID, io).await?;
            }
        }
        Ok(())
    }

    fn bit_set<const S: usize>(block: &mut [u8; S], bit: usize) {
        let byte = bit / 8;
        let mask = 1u8 << (bit % 8);
        block[byte] |= mask;
    }

    fn bit_clear<const S: usize>(block: &mut [u8; S], bit: usize) {
        let byte = bit / 8;
        let mask = 1u8 << (bit % 8);
        block[byte] &= !mask;
    }

    #[bisync]
    pub(crate) async fn mark_allocated(
        &mut self,
        io: &mut D,
        run: &AllocatedRun,
        allocated: bool,
    ) -> Result<(), ExFatError<D>> {
        let first_sector = self.bitmap.first_sector;
        let num_sectors = self.bitmap.num_sectors;

        let sector_index = (run.first_cluster - FIRST_CLUSTER_ID) / BLOCK_SIZE as u32;
        let mut remaining = run.cluster_count;
        let mut cluster_id = run.first_cluster;

        for sector_offset in sector_index..num_sectors {
            let slot = self.cache.read(first_sector + sector_offset, io).await?;

            let first_cluster_of_slot = sector_index * BLOCK_SIZE as u32 + FIRST_CLUSTER_ID;
            let start = cluster_id - first_cluster_of_slot;
            let end = (BLOCK_SIZE as u32).min(start + remaining);

            if allocated {
                for bit in start..end {
                    Self::bit_set(&mut slot.block, bit as usize);
                }
            } else {
                for bit in start..end {
                    Self::bit_clear(&mut slot.block, bit as usize);
                }
            }

            let num_clusters_in_slot = end - start;
            remaining -= num_clusters_in_slot;
            cluster_id += num_clusters_in_slot;

            if remaining == 0 {
                break;
            }

            //let start
        }

        Ok(())
    }

    /*
        /// locates the next free set of contiguous clusters.
        /// if from_cluster is Some it will, like all clusters, only be included if it is NOT allocated.
        #[bisync]
        async fn find_free_clusters_contiguous_from_old(
            &mut self,
            fs: &mut FileSystem<D>,
            from_cluster: u32,
            num_clusters: u32,
        ) -> Result<Option<AllocatedRun>, ExFatError<D>> {
            let sector_index = (from_cluster - FIRST_CLUSTER_ID) / BLOCK_SIZE as u32;
            let mut cluster_id = sector_index * BLOCK_SIZE as u32 + FIRST_CLUSTER_ID;
            let mut first_cluster = None;
            let mut count = 0;

            crate::info!(
                "find_free_clusters_contiguous_from num_clusters {} from_cluster {}",
                num_clusters,
                from_cluster,
            );

            for sector_offset in sector_index..self.bitmap.num_sectors {
                let slot = self
                    .cache
                    .read(self.bitmap.first_sector + sector_offset, fs)
                    .await?;
                /*
                fs.dev
                    .read(sector_id + sector_offset, &mut block)
                    .await
                    .map_err(ExFatError::Io)?;
                */
                let (bytes, _remainder) = slot.block.as_chunks::<4>();
                for chunk in bytes {
                    if u32::from_le_bytes(*chunk) != u32::MAX {
                        for byte in chunk {
                            for bit in 0..u8::BITS {
                                if *byte & 1 << bit == 0 {
                                    if cluster_id < from_cluster {
                                        continue;
                                    }

                                    if from_cluster == cluster_id {
                                        first_cluster = Some(cluster_id);
                                        count = 1;
                                    } else if first_cluster.is_none() {
                                        return Ok(None);
                                    } else {
                                        count += 1;
                                    }

                                    if count == num_clusters {
                                        crate::info!("first_cluster {:?}", first_cluster);
                                        return Ok(Some(AllocatedRun {
                                            cluster_id,
                                            count: num_clusters,
                                        }));
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

            Ok(None)
        }
    */
    /// locates the next free set of contiguous clusters.
    /// if from_cluster is Some it will, like all clusters, only be included if it is NOT allocated.
    #[bisync]
    async fn find_free_clusters_from(
        &mut self,
        io: &mut D,
        from_cluster: u32,
        num_clusters: u32,
    ) -> Result<AllocatedRun, ExFatError<D>> {
        let first_sector = self.bitmap.first_sector;
        let num_sectors = self.bitmap.num_sectors;

        let sector_index = (from_cluster - FIRST_CLUSTER_ID) / BLOCK_SIZE as u32;
        let mut cluster_id = sector_index * BLOCK_SIZE as u32 + FIRST_CLUSTER_ID;
        let mut first_cluster = None;
        let mut count = 0;

        for sector_offset in sector_index..num_sectors {
            let slot = self.cache.read(first_sector + sector_offset, io).await?;

            let (bytes, _remainder) = slot.block.as_chunks::<4>();
            for chunk in bytes {
                if u32::from_le_bytes(*chunk) != u32::MAX {
                    for byte in chunk {
                        for bit in 0..u8::BITS {
                            if *byte & 1 << bit == 0 {
                                if cluster_id < from_cluster {
                                    continue;
                                }

                                if first_cluster.is_none() {
                                    first_cluster = Some(cluster_id);
                                    count = 1;
                                } else {
                                    count += 1;
                                }

                                if count == num_clusters {
                                    return Ok(AllocatedRun {
                                        first_cluster: cluster_id,
                                        cluster_count: num_clusters,
                                    });
                                }
                            } else {
                                if let Some(first_cluster) = first_cluster {
                                    return Ok(AllocatedRun {
                                        first_cluster: first_cluster,
                                        cluster_count: count,
                                    });
                                }
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

    /// locates the next free set of contiguous clusters.
    /// this is used for creating a new file
    #[bisync]
    pub(crate) async fn find_free_clusters(
        &mut self,
        io: &mut D,
        num_clusters: u32,
    ) -> Result<AllocatedRun, ExFatError<D>> {
        let mut cluster_id = self.next_search_cluster;
        let sector_id = self.bitmap.first_sector;
        let num_sectors = self.bitmap.num_sectors;
        let sector_index = (cluster_id - FIRST_CLUSTER_ID) / BLOCK_SIZE as u32;
        let mut first_cluster = None;
        let mut count = 0;

        let mut fallback = None;

        for sector_offset in sector_index..num_sectors {
            let slot = self.cache.read(sector_id + sector_offset, io).await?;
            let (bytes, _remainder) = slot.block.as_chunks::<4>();
            for chunk in bytes {
                if u32::from_le_bytes(*chunk) != u32::MAX {
                    for byte in chunk {
                        for bit in 0..u8::BITS {
                            if *byte & 1 << bit == 0 {
                                if first_cluster.is_none() {
                                    first_cluster = Some(cluster_id);
                                    count = 1
                                } else {
                                    count += 1;
                                }

                                if count == num_clusters {
                                    let first = first_cluster.unwrap();
                                    self.next_search_cluster = first + count;
                                    return Ok(AllocatedRun {
                                        first_cluster: first,
                                        cluster_count: num_clusters,
                                    });
                                }
                            } else {
                                if fallback.is_none() {
                                    if let Some(cluster_id) = first_cluster {
                                        fallback = Some(AllocatedRun {
                                            first_cluster: cluster_id,
                                            cluster_count: num_clusters,
                                        })
                                    }
                                }
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

        self.next_search_cluster = FIRST_CLUSTER_ID;
        match fallback {
            Some(run) => Ok(run),
            None => Err(ExFatError::DiskFull),
        }
    }

    /*
    /// locates the next free set of contiguous clusters.
    #[bisync]
    pub(crate) async fn find_free_clusters_contiguous_old(
        &mut self,
        fs: &mut FileSystem<D>,
        num_clusters: u32,
    ) -> Result<Option<AllocatedRun>, ExFatError<D>> {
        let mut cluster_id = self.next_search_cluster;
        let sector_id = self.bitmap.first_sector;
        let sector_index = (cluster_id - FIRST_CLUSTER_ID) / BLOCK_SIZE as u32;
        let mut first_cluster = None;
        let mut count = 0;

        for sector_offset in sector_index..self.bitmap.num_sectors {
            let slot = self.cache.read(sector_id + sector_offset, fs).await?;
            let (bytes, _remainder) = slot.block.as_chunks::<4>();
            for chunk in bytes {
                if u32::from_le_bytes(*chunk) != u32::MAX {
                    for byte in chunk {
                        for bit in 0..u8::BITS {
                            if *byte & 1 << bit == 0 {
                                if first_cluster.is_none() {
                                    first_cluster = Some(cluster_id);
                                    count = 1
                                } else {
                                    count += 1;
                                }

                                if count == num_clusters {
                                    let first = first_cluster.unwrap();
                                    self.next_search_cluster = first + count;
                                    return Ok(Some(AllocatedRun {
                                        cluster_id,
                                        count: num_clusters,
                                    }));
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

        self.next_search_cluster = FIRST_CLUSTER_ID;
        Ok(None)
    }
    */
}

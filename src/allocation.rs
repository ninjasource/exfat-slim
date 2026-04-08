use core::marker::PhantomData;

use crate::asynchronous::io::BLOCK_SIZE;

use super::bisync;
use super::error::ExFatError;
use super::file_system::FileSystem;
use super::io::BlockDevice;

const FIRST_CLUSTER_ID: u32 = 2;

pub struct AllocatedRun {
    pub first: u32,
    pub count: u32,
}

pub enum StoredChain {
    Empty,
    Contiguous { first: u32, len: u32 },
    Fat { first: u32, last: u32, len: u32 },
}

pub struct Allocator<D: BlockDevice> {
    alloc_bitmap_first_sector: u32,
    alloc_bitmap_num_sectors: u32,
    next_search_cluster: u32,
    _phantom: PhantomData<D>,
}

impl<D: BlockDevice> Allocator<D> {
    #[bisync]
    pub async fn alloc_clusters(&mut self, count: u32) -> Result<AllocatedRun, ExFatError<D>> {
        panic!()
    }

    #[bisync]
    pub async fn alloc_clusters_append(
        &mut self,
        chain: &StoredChain,
        count: u32,
    ) -> Result<AllocatedRun, ExFatError<D>> {
        panic!()
    }

    #[bisync]
    pub async fn free_clusters(&mut self, chain: &StoredChain) -> Result<(), ExFatError<D>> {
        Ok(())
    }

    /// locates the next free set of contiguous clusters.
    /// if from_cluster is Some it will, like all clusters, only be included if it is NOT allocated.
    #[bisync]
    async fn find_free_clusters_contiguous_from(
        &self,
        fs: &mut FileSystem<D>,
        from_cluster: u32,
        num_clusters: u32,
    ) -> Result<AllocatedRun, ExFatError<D>> {
        let sector_id = self.alloc_bitmap_first_sector;

        //let sector_id = fs.get_heap_sector_id(from_cluster)?;
        let sector_index = (from_cluster - FIRST_CLUSTER_ID) / BLOCK_SIZE as u32;
        let mut cluster_id = sector_index * BLOCK_SIZE as u32 + FIRST_CLUSTER_ID;
        let mut first_cluster = None;
        let mut count = 0;
        let mut block = [0u8; BLOCK_SIZE];

        crate::info!(
            "find_free_clusters_contiguous_from num_clusters {} from_cluster {}",
            num_clusters,
            from_cluster,
        );

        for sector_offset in sector_index..self.alloc_bitmap_num_sectors {
            fs.dev
                .read(sector_id + sector_offset, &mut block)
                .await
                .map_err(ExFatError::Io)?;
            let (bytes, _remainder) = block.as_chunks::<4>();
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
                                    return Err(ExFatError::DiskFull);
                                } else {
                                    count += 1;
                                }

                                if count == num_clusters {
                                    crate::info!("first_cluster {:?}", first_cluster);
                                    let first = first_cluster.unwrap();
                                    return Ok(AllocatedRun {
                                        first,
                                        count: num_clusters,
                                    });
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

    /// locates the next free set of contiguous clusters.
    #[bisync]
    pub(crate) async fn find_free_clusters_contiguous(
        &mut self,
        fs: &mut FileSystem<D>,
        num_clusters: u32,
    ) -> Result<AllocatedRun, ExFatError<D>> {
        let mut cluster_id = self.next_search_cluster;
        let sector_id = self.alloc_bitmap_first_sector;
        let sector_index = (cluster_id - FIRST_CLUSTER_ID) / BLOCK_SIZE as u32;
        let mut first_cluster = None;
        let mut count = 0;
        let mut block = [0u8; BLOCK_SIZE];

        for sector_offset in sector_index..self.alloc_bitmap_num_sectors {
            fs.dev
                .read(sector_id + sector_offset, &mut block)
                .await
                .map_err(ExFatError::Io)?;
            let (bytes, _remainder) = block.as_chunks::<4>();
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
                                        first,
                                        count: num_clusters,
                                    });
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
        Err(ExFatError::DiskFull)
    }
}

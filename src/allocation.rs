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
/// Quote from the microsoft spec section 7.1.5 Note "The first bit in the bitmap is the lowest-order bit of the first byte."
///
/// NOTE: I encountered what appears to be a logic error in the linux kernel exfat implementation where bits are incorrectly counted from MSB to LSB and not the other way around when locating free allocations.
/// this does not affect the allocation bitmap write consistency but could possibly lead to unnecessary fragmentation
///
use core::{marker::PhantomData, ops::Range};

use super::{
    BlockDevice, bisync,
    directory_entry::AllocationBitmapDirEntry,
    error::ExFatError,
    fat::Fat,
    file::{NO_CLUSTER_ID, Touched, TouchedKind, TouchedSector},
    file_system::ExFatResult,
    slot_cache::SlotCache,
};

const FIRST_CLUSTER_ID: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BitmapPos<const SIZE: usize> {
    /// 0 based sector number within the bitmap
    pub sector: u32,
    /// bit index within the sector
    pub bit: u32,
}

impl<const SIZE: usize> BitmapPos<SIZE> {
    const CLUSTERS_PER_SECTOR: u32 = (SIZE * 8) as u32;

    const fn of(cluster_id: u32) -> Self {
        let index = cluster_id - FIRST_CLUSTER_ID;
        Self {
            sector: index / Self::CLUSTERS_PER_SECTOR,
            bit: index % Self::CLUSTERS_PER_SECTOR,
        }
    }

    const fn cluster(self) -> u32 {
        self.sector * Self::CLUSTERS_PER_SECTOR + self.bit + FIRST_CLUSTER_ID
    }

    const fn first_cluster_of(sector: u32) -> u32 {
        Self { sector, bit: 0 }.cluster()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BitmapChunk {
    /// 0 based sector number within the bitmap
    pub sector: u32,
    /// the bit range to set or clear within that sector
    pub bits: Range<u32>,
}

pub(crate) struct BitmapRunChunks<const SIZE: usize> {
    cluster_id: u32,
    remaining: u32,
}

impl<const SIZE: usize> BitmapRunChunks<SIZE> {
    pub(crate) const fn new(first_cluster: u32, cluster_count: u32) -> Self {
        Self {
            cluster_id: first_cluster,
            remaining: cluster_count,
        }
    }
}

impl<const SIZE: usize> Iterator for BitmapRunChunks<SIZE> {
    type Item = BitmapChunk;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        let pos = BitmapPos::<SIZE>::of(self.cluster_id);
        let taken = (BitmapPos::<SIZE>::CLUSTERS_PER_SECTOR - pos.bit).min(self.remaining);

        self.cluster_id += taken;
        self.remaining -= taken;

        Some(BitmapChunk {
            sector: pos.sector,
            bits: pos.bit..pos.bit + taken,
        })
    }
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, Clone)]
pub(crate) struct AllocationBitmap<const SIZE: usize> {
    /// start of the allocation bitmap table
    pub first_cluster: u32,
    /// size, in sectors, of the allocation bitmap
    pub num_sectors: u32,
}

impl<const SIZE: usize> AllocationBitmap<SIZE> {
    pub(crate) fn new(alloc_bitmap: &AllocationBitmapDirEntry) -> Self {
        let num_sectors = alloc_bitmap.data_length.div_ceil(SIZE as u64) as u32;

        Self {
            first_cluster: alloc_bitmap.first_cluster,
            num_sectors,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchPolicy {
    /// stop at the first free run even if it is shorter than the one asked for.
    /// typically used when a fat chain is already in use
    FirstRun,

    /// scan the entire bitmap for a contiguous run and wrap around to the beginning if necessary
    /// ususally used for new files where we want to attempt NoFatChain
    LongestRun,
}

pub(crate) struct AllocatedRun {
    pub first_cluster: u32,
    pub cluster_count: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum StoredChain {
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
pub(crate) struct AllocationBitmapSlim {
    pub first_sector: u32,
    pub num_sectors: u32,
    // from boot sector cluster_count
    // bits in the bitmap past this are padding and should never be allocated
    pub cluster_count: u32,
}

#[derive(Debug)]
pub(crate) struct Allocator<D, const SIZE: usize, const N: usize>
where
    D: BlockDevice<SIZE>,
{
    pub bitmap: AllocationBitmapSlim,
    cache: SlotCache<D, SIZE, N>,
    next_search_cluster: u32,
    _phantom: PhantomData<D>,
}

impl<D, const SIZE: usize, const N: usize> Allocator<D, SIZE, N>
where
    D: BlockDevice<SIZE>,
{
    pub fn new() -> Self {
        Self {
            bitmap: AllocationBitmapSlim::default(),
            cache: SlotCache::new(),
            next_search_cluster: FIRST_CLUSTER_ID,
            _phantom: PhantomData,
        }
    }

    /// closes a run in progress.
    /// if the policy says that we need to stop scanning then it will return Some, otherwise None
    fn close_run(
        policy: SearchPolicy,
        longest: &mut Option<AllocatedRun>,
        first_cluster: &mut Option<u32>,
        count: &mut u32,
    ) -> Option<AllocatedRun> {
        let run = first_cluster.take().map(|first_cluster| AllocatedRun {
            first_cluster,
            cluster_count: *count,
        });
        *count = 0;

        match (policy, run) {
            (SearchPolicy::FirstRun, run) => run,
            (SearchPolicy::LongestRun, Some(run)) => {
                if longest
                    .as_ref()
                    .is_none_or(|x| x.cluster_count < run.cluster_count)
                {
                    *longest = Some(run);
                }

                None
            }
            (SearchPolicy::LongestRun, None) => None,
        }
    }

    #[bisync]
    pub async fn flush_sector(&mut self, io: &mut D, sector: u32) -> ExFatResult<(), D, SIZE> {
        self.cache.flush_sector(io, sector).await?;
        Ok(())
    }

    #[bisync]
    pub async fn flush(&mut self, io: &mut D) -> ExFatResult<(), D, SIZE> {
        self.cache.flush(io).await?;
        Ok(())
    }

    #[bisync]
    pub async fn allocate(
        &mut self,
        io: &mut D,
        touched: &mut impl Touched,
        chain: &StoredChain,
        count: u32,
    ) -> ExFatResult<AllocatedRun, D, SIZE> {
        let run = match chain {
            StoredChain::Empty => {
                self.find_free_clusters(io, None, count, SearchPolicy::LongestRun)
                    .await?
            }

            StoredChain::Contiguous {
                first,
                cluster_count,
            } => {
                self.find_free_clusters(
                    io,
                    Some(first + cluster_count),
                    count,
                    SearchPolicy::FirstRun,
                )
                .await?
            }
            StoredChain::Fat { last, .. } => {
                self.find_free_clusters(io, Some(last + 1), count, SearchPolicy::FirstRun)
                    .await?
            }
        };

        self.mark_allocated(io, touched, &run, true).await?;
        Ok(run)
    }

    #[bisync]
    pub async fn free(
        &mut self,
        io: &mut D,
        touched: &mut impl Touched,
        fat: &mut Fat<D, SIZE, N>,
        chain: &StoredChain,
    ) -> ExFatResult<(), D, SIZE> {
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
                self.mark_allocated(io, touched, &run, false).await?
            }
            StoredChain::Fat {
                first,
                last: _last,
                cluster_count: _cluster_count,
            } => {
                let mut cluster_id = *first;

                // bounded by cluster count so that we cant get in an infinite
                // loop with a cycle caused by a corrupt fat chain
                for _ in 0..*_cluster_count {
                    let next_cluster_id = fat.next_cluster_in_fat_chain(cluster_id, io).await?;

                    let run = AllocatedRun {
                        first_cluster: cluster_id,
                        cluster_count: 1,
                    };

                    self.mark_allocated(io, touched, &run, false).await?;
                    fat.set(io, touched, cluster_id, NO_CLUSTER_ID).await?;

                    match next_cluster_id {
                        Some(next) => cluster_id = next,
                        None => break,
                    }
                }
            }
        }
        Ok(())
    }

    fn set_bit_range(block: &mut [u8], range: Range<u32>, set: bool) {
        if set {
            for bit in range {
                // set the bit
                let byte = bit as usize / 8;
                let mask = 1u8 << (bit % 8);
                block[byte] |= mask;
            }
        } else {
            for bit in range {
                // clear the bit
                let byte = bit as usize / 8;
                let mask = 1u8 << (bit % 8);
                block[byte] &= !mask;
            }
        }
    }

    #[bisync]
    pub(crate) async fn mark_allocated(
        &mut self,
        io: &mut D,
        touched: &mut impl Touched,
        run: &AllocatedRun,
        allocated: bool,
    ) -> ExFatResult<(), D, SIZE> {
        if run.cluster_count == 0 {
            return Ok(());
        }

        if run.first_cluster < FIRST_CLUSTER_ID {
            return Err(ExFatError::InvalidClusterId(run.first_cluster));
        }

        // check that the whole run fits in allocation bitmap
        // remember that BitmapPos is relative to the start of the bitmap so we can use num_sectors
        //let last_cluster = run.first_cluster + run.cluster_count - 1;
        let Some(last_cluster) = run.first_cluster.checked_add(run.cluster_count - 1) else {
            return Err(ExFatError::InvalidClusterId(run.first_cluster));
        };
        if last_cluster >= FIRST_CLUSTER_ID + self.bitmap.cluster_count
            || BitmapPos::<SIZE>::of(last_cluster).sector >= self.bitmap.num_sectors
        {
            return Err(ExFatError::InvalidClusterId(run.first_cluster));
        }

        for chunk in BitmapRunChunks::<SIZE>::new(run.first_cluster, run.cluster_count) {
            let sector_id = self.bitmap.first_sector + chunk.sector;
            let slot = self.cache.read_mut(sector_id, io).await?;
            Self::set_bit_range(slot.as_mut_slice(), chunk.bits, allocated);
            touched.insert(TouchedSector::new(TouchedKind::Bitmap, sector_id));
        }

        Ok(())
    }

    #[bisync]
    async fn find_free_clusters_from(
        &mut self,
        io: &mut D,
        from_cluster: u32,
        num_clusters: u32,
        policy: SearchPolicy,
    ) -> ExFatResult<AllocatedRun, D, SIZE> {
        if from_cluster < FIRST_CLUSTER_ID {
            return Err(ExFatError::InvalidClusterId(from_cluster));
        }

        let first_sector = self.bitmap.first_sector;
        let num_sectors = self.bitmap.num_sectors;
        let end_cluster = FIRST_CLUSTER_ID + self.bitmap.cluster_count;
        let start = BitmapPos::<SIZE>::of(from_cluster);

        if from_cluster >= end_cluster || start.sector >= num_sectors {
            // nothing free after from_cluster.
            // let the called decide wether to wrap
            return Err(ExFatError::DiskFull);
        }

        let mut cluster_id = BitmapPos::<SIZE>::first_cluster_of(start.sector);
        let mut first_cluster = None;
        let mut count = 0;
        let mut longest = None;

        'scan: for sector in start.sector..num_sectors {
            let slot = self.cache.read(first_sector + sector, io).await?;
            let (chunks, _remainder) = slot.as_slice().as_chunks::<4>();

            for chunk in chunks {
                if u32::from_le_bytes(*chunk) == u32::MAX {
                    if let Some(run) =
                        Self::close_run(policy, &mut longest, &mut first_cluster, &mut count)
                    {
                        return Ok(run);
                    }

                    cluster_id += u32::BITS;
                    continue;
                }

                for byte in chunk {
                    for bit in 0..u8::BITS {
                        // we start scanning at the start of the sector so we need to skip all those sectors
                        if cluster_id < from_cluster {
                            cluster_id += 1;
                            continue;
                        }

                        if cluster_id >= end_cluster {
                            // everything from here is just padding
                            break 'scan;
                        }

                        if is_free(*byte, bit) {
                            if first_cluster.is_none() {
                                first_cluster = Some(cluster_id)
                            }
                            count += 1;

                            if count == num_clusters {
                                return Ok(AllocatedRun {
                                    first_cluster: first_cluster.unwrap(),
                                    cluster_count: count,
                                });
                            }
                        } else if let Some(run) =
                            Self::close_run(policy, &mut longest, &mut first_cluster, &mut count)
                        {
                            return Ok(run);
                        }

                        cluster_id += 1;
                    }
                }
            }
        }

        if let Some(run) = Self::close_run(policy, &mut longest, &mut first_cluster, &mut count) {
            return Ok(run);
        }

        longest.ok_or(ExFatError::DiskFull)
    }

    /// locates the next free set of contiguous clusters.
    /// this is used for creating a new file
    #[bisync]
    pub(crate) async fn find_free_clusters(
        &mut self,
        io: &mut D,
        from_cluster: Option<u32>,
        num_clusters: u32,
        policy: SearchPolicy,
    ) -> ExFatResult<AllocatedRun, D, SIZE> {
        let from_cluster = from_cluster.unwrap_or(self.next_search_cluster);
        let run = match self
            .find_free_clusters_from(io, from_cluster, num_clusters, policy)
            .await
        {
            Ok(run) => run,
            Err(ExFatError::DiskFull) if from_cluster > FIRST_CLUSTER_ID => {
                self.find_free_clusters_from(io, FIRST_CLUSTER_ID, num_clusters, policy)
                    .await?
            }
            Err(e) => return Err(e),
        };

        self.next_search_cluster = run.first_cluster + run.cluster_count;
        Ok(run)
    }
}

const fn is_free(byte: u8, bit_in_byte: u32) -> bool {
    byte & (1 << bit_in_byte) == 0
}

#[allow(unused)]
#[cfg(test)]
mod tests {
    use crate::blocking::file::FileDirty;
    use crate::test_utils::{BLOCK_SIZE, DummyBlockDevice};

    use super::super::only_sync;
    use super::*;

    #[only_sync]
    #[test]
    fn alloate_and_free_bitmap_bits() {
        let mut io = DummyBlockDevice::new(4);
        let mut alloc = Allocator::<DummyBlockDevice, BLOCK_SIZE, 4>::new();
        alloc.bitmap.first_sector = 1;
        alloc.bitmap.num_sectors = 1;
        alloc.bitmap.cluster_count = BLOCK_SIZE as u32 * 8; // heap fills the entire bitmap sector
        let mut touched = FileDirty::new();

        // act
        let run = alloc
            .allocate(&mut io, &mut touched, &StoredChain::Empty, 10)
            .unwrap();
        alloc.flush(&mut io).unwrap();

        // assert
        assert_eq!(run.first_cluster, 2);
        assert_eq!(run.cluster_count, 10);
        assert_eq!(io.blocks[1][0], 0b1111_1111);
        assert_eq!(io.blocks[1][1], 0b0000_0011); // note the order of the bits here (in line with how windows does it)
        assert!(io.blocks[1][2..].iter().all(|b| *b == 0)); // all the rest are unallocated
        assert!(io.blocks[0].iter().all(|b| *b == 0)); // first sector untouched
    }

    #[only_sync]
    #[test]
    fn allocate_run_crossing_bitmap_sector_boundary() {
        let mut io = DummyBlockDevice::new(4);
        let mut alloc = Allocator::<DummyBlockDevice, BLOCK_SIZE, 4>::new();
        alloc.bitmap.first_sector = 1;
        alloc.bitmap.num_sectors = 2;
        alloc.bitmap.cluster_count = alloc.bitmap.num_sectors * BLOCK_SIZE as u32 * 8; // heap fills the entire bitmap sector
        let mut touched = FileDirty::new();
        let run = AllocatedRun {
            first_cluster: 4094,
            cluster_count: 8,
        };

        // act
        alloc
            .mark_allocated(&mut io, &mut touched, &run, true)
            .unwrap();
        alloc.flush(&mut io).unwrap();

        // assert
        assert_eq!(io.blocks[1][511], 0b1111_0000);
        assert_eq!(io.blocks[2][0], 0b0000_1111);
    }

    #[only_sync]
    #[test]
    fn cluster_and_bitmap_position_round_trip() {
        for cluster in (2..6).chain(4094..4102).chain(8190..8198) {
            let pos = BitmapPos::<BLOCK_SIZE>::of(cluster);
            assert_eq!(pos.cluster(), cluster, "round trip for cluster {cluster}");
        }

        assert_eq!(
            BitmapPos::<BLOCK_SIZE>::of(2),
            BitmapPos { sector: 0, bit: 0 }
        );
        assert_eq!(
            BitmapPos::<BLOCK_SIZE>::of(4097),
            BitmapPos {
                sector: 0,
                bit: 4095
            }
        );
        assert_eq!(
            BitmapPos::<BLOCK_SIZE>::of(4098),
            BitmapPos { sector: 1, bit: 0 }
        );
    }

    #[only_sync]
    #[test]
    fn run_splits_at_bitmap_sector_boundaries() {
        let chunks: Vec<_> = BitmapRunChunks::<BLOCK_SIZE>::new(4094, 8).collect();
        assert_eq!(
            chunks,
            vec![
                BitmapChunk {
                    sector: 0,
                    bits: 4092..4096
                },
                BitmapChunk {
                    sector: 1,
                    bits: 0..4
                }
            ]
        );

        // single sector, the very first clusters
        assert_eq!(
            BitmapRunChunks::<BLOCK_SIZE>::new(2, 10).collect::<Vec<_>>(),
            vec![BitmapChunk {
                sector: 0,
                bits: 0..10
            }]
        );

        // some edge cases. also check that the cluster count requested matches what we actually get
        for (first_cluster, cluster_count) in
            [(2, 1), (4094, 8), (4097, 2), (4098, 4096), (5000, 9000)]
        {
            let total: u32 = BitmapRunChunks::<BLOCK_SIZE>::new(first_cluster, cluster_count)
                .map(|x| x.bits.len() as u32)
                .sum();
            assert_eq!(
                total, cluster_count,
                "cluster_id {first_cluster} count {cluster_count}"
            );
        }
    }

    #[only_sync]
    #[test]
    fn do_not_allocate_past_end_of_cluster_heap() {
        let mut io = DummyBlockDevice::new(4);
        let mut alloc = Allocator::<DummyBlockDevice, BLOCK_SIZE, 4>::new();
        alloc.bitmap.first_sector = 1;
        alloc.bitmap.num_sectors = 1;
        alloc.bitmap.cluster_count = 10; // clusters 2..=11 (the rest of the sector is padding)
        let mut touched = FileDirty::new();

        // allocate all 10 available clusters
        let run = alloc
            .allocate(&mut io, &mut touched, &StoredChain::Empty, 12)
            .unwrap();
        assert_eq!(run.first_cluster, 2);
        assert_eq!(run.cluster_count, 10);

        // allocate one more
        assert!(matches!(
            alloc.allocate(&mut io, &mut touched, &StoredChain::Empty, 1),
            Err(ExFatError::DiskFull)
        ));

        // attempt to mark allocated past valid cluster
        let run = AllocatedRun {
            first_cluster: 12,
            cluster_count: 1,
        };
        assert!(matches!(
            alloc.mark_allocated(&mut io, &mut touched, &run, true),
            Err(ExFatError::InvalidClusterId(12))
        ));
    }

    #[only_sync]
    #[test]
    fn search_wraps_to_start_of_heap_when_end_is_full() {
        let mut io = DummyBlockDevice::new(4);
        let mut alloc = Allocator::<DummyBlockDevice, BLOCK_SIZE, 4>::new();
        alloc.bitmap.first_sector = 1;
        alloc.bitmap.num_sectors = 1;
        alloc.bitmap.cluster_count = 8; // clusters 2..=9 (the rest of the sector is padding)
        let mut touched = FileDirty::new();

        // fill the heap then free the first 3 clusters
        alloc
            .allocate(&mut io, &mut touched, &StoredChain::Empty, 8)
            .unwrap();
        let freed = AllocatedRun {
            first_cluster: 2,
            cluster_count: 3,
        };
        alloc
            .mark_allocated(&mut io, &mut touched, &freed, false)
            .unwrap();

        // the internal next_search_cluster cursor will be at 10 so the search should wrap
        let run = alloc
            .allocate(&mut io, &mut touched, &StoredChain::Empty, 2)
            .unwrap();
        assert_eq!(run.first_cluster, 2);
        assert_eq!(run.cluster_count, 2);
    }
}

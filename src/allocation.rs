use core::marker::PhantomData;

use super::error::ExFatError;

use super::io::BlockDevice;

pub struct Cluster(u32);

pub struct AllocatedRun {
    pub first: Cluster,
    pub count: usize,
}

enum StoredChain {
    Empty,
    Contiguous {
        first: Cluster,
        len: u32,
    },
    Fat {
        first: Cluster,
        last: Cluster,
        len: u32,
    },
}

pub struct Allocator<D: BlockDevice> {
    _phantom: PhantomData<D>,
}

impl<D: BlockDevice> Allocator<D> {
    pub async fn new_allocation(&mut self, count: u32) -> Result<AllocatedRun, ExFatError<D>> {
        panic!()
    }

    pub async fn append(
        &mut self,
        chain: &StoredChain,
        count: u32,
    ) -> Result<AllocatedRun, ExFatError<D>> {
        panic!()
    }

    pub async fn free(&mut self, chain: &StoredChain) -> Result<(), ExFatError<D>> {
        Ok(())
    }
}

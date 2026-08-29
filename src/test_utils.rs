use crate::blocking::{
    BlockDevice, directory::MAX_NAME_LEN, file::OpenOptions, file_system::FileSystem,
    upcase_table::UpcaseTable,
};

use super::*;
use aligned::Aligned;
use alloc::vec::Vec;

pub(crate) const SECTOR_OFFSET: usize = 0;
pub(crate) const BLOCK_SIZE: usize = 512;

#[derive(Debug)]
pub(crate) struct DummyBlockDevice {
    pub blocks: Vec<[u8; BLOCK_SIZE]>,
    pub write_count: usize,
}

impl DummyBlockDevice {
    pub fn new(count: usize) -> Self {
        Self {
            blocks: vec![[0u8; BLOCK_SIZE]; count],
            write_count: 0,
        }
    }
}

impl BlockDevice<BLOCK_SIZE> for DummyBlockDevice {
    type Error = ();
    type Align = aligned::A4;

    fn read(
        &mut self,
        block_address: u32,
        data: &mut [Aligned<Self::Align, [u8; BLOCK_SIZE]>],
    ) -> Result<(), Self::Error> {
        data[0].copy_from_slice(&self.blocks[block_address as usize - SECTOR_OFFSET]);
        Ok(())
    }

    fn write(
        &mut self,
        block_address: u32,
        data: &[Aligned<Self::Align, [u8; BLOCK_SIZE]>],
    ) -> Result<(), Self::Error> {
        self.blocks[block_address as usize - SECTOR_OFFSET].copy_from_slice(data[0].as_slice());
        self.write_count += 1;
        Ok(())
    }

    fn size(&mut self) -> Result<u64, Self::Error> {
        todo!()
    }
}

pub(crate) fn read_file(
    fs: &mut FileSystem<DummyBlockDevice, BLOCK_SIZE, 4>,
    path: &str,
) -> Vec<u8> {
    let options = OpenOptions::new().read(true);
    let mut file = fs.open(path, options).unwrap();
    let len = file.metadata().len();
    let mut buf = vec![0u8; len as usize];
    file.read(fs, &mut buf).unwrap();
    buf
}

pub(crate) fn empty_fs() -> FileSystem<DummyBlockDevice, BLOCK_SIZE, 4> {
    let mut io = DummyBlockDevice::new(512); // 8 clusters
    io.blocks[1][0] = 1; // cluster 2 (root dir) allocated
    let mut fs = FileSystem::<_, _, 4>::new(io);
    fs.is_mounted = true;
    fs.fs.first_cluster_of_root_dir = 2;
    fs.upcase_table = UpcaseTable::default();
    fs.allocator.bitmap.first_sector = 1;
    fs.allocator.bitmap.num_sectors = 1;
    fs.allocator.bitmap.cluster_count = 8;
    fs
}

pub(crate) fn list_names(
    fs: &mut FileSystem<DummyBlockDevice, BLOCK_SIZE, 4>,
    path: &str,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut name_buf = [0u8; MAX_NAME_LEN];
    let mut iter = fs.read_dir(path).unwrap();
    while let Some(entry) = iter.next_entry(fs, &mut name_buf).unwrap() {
        names.push(String::from(entry.name));
    }

    names
}

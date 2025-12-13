use std::fs;

use exfat_slim::io::{BLOCK_SIZE, Block, BlockDevice, Error};
use flate2::read::GzDecoder;
use log::trace;
use std::io::Read;

pub struct InMemoryBlockDevice {
    pub image: Vec<u8>,
    pub sector_offset: u32,
    pub data_block: [u8; BLOCK_SIZE],
    last_sector: Option<u32>,
}

impl BlockDevice for InMemoryBlockDevice {
    async fn read_sector(&mut self, sector_id: u32) -> Result<&Block, Error> {
        let sector_id_with_offset = sector_id + self.sector_offset;
        match self.last_sector.as_ref() {
            Some(x) if *x == sector_id_with_offset => {
                print("READ", sector_id_with_offset, sector_id, true)
            }
            _ => {
                print("READ", sector_id_with_offset, sector_id, false);
                let pos = sector_id_with_offset as usize * BLOCK_SIZE;
                self.data_block
                    .copy_from_slice(&self.image[pos..pos + BLOCK_SIZE]);
                self.last_sector = Some(sector_id_with_offset)
            }
        }

        Ok(&self.data_block)
    }

    async fn write_sector(&mut self, sector_id: u32, block: &Block) -> Result<(), Error> {
        let sector_id_with_offset = sector_id + self.sector_offset;
        print("WRITE", sector_id_with_offset, sector_id, false);
        let pos = sector_id_with_offset as usize * BLOCK_SIZE;
        self.image[pos..pos + BLOCK_SIZE].copy_from_slice(block);
        self.last_sector = None;
        Ok(())
    }
}

fn print(direction: &str, sector_id_with_offset: u32, sector_id: u32, is_cached: bool) {
    let cached = if is_cached { " (cached)" } else { "" };
    if sector_id >= 4096 {
        let cluster_id = ((sector_id - 4096) / 64) + 2;
        trace!(
            "{} sector {} cluster {}{}",
            direction, sector_id_with_offset, cluster_id, cached
        );
    } else {
        trace!("{} sector {}{}", direction, sector_id_with_offset, cached);
    }
}

impl InMemoryBlockDevice {
    pub fn new() -> Self {
        // this sd card image is 10MB uncompressed
        let buf = fs::read("./examples/common/sd.img.gz").unwrap();
        let mut reader = GzDecoder::new(buf.as_slice());
        let mut image = Vec::new();
        reader.read_to_end(&mut image).unwrap();

        Self {
            image,
            sector_offset: 0,
            data_block: [0; BLOCK_SIZE],
            last_sector: None,
        }
    }
}

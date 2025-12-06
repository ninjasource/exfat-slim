use std::fs;

use exfat_slim::io::{BLOCK_SIZE, Block, BlockDevice, Error};
use flate2::read::GzDecoder;
use log::info;
use std::io::Read;

pub struct InMemoryBlockDevice {
    pub image: Vec<u8>,
    pub sector_offset: u32,
    pub data_block: [u8; BLOCK_SIZE],
}

impl BlockDevice for InMemoryBlockDevice {
    async fn read_sector(&mut self, sector_id: u32) -> Result<&Block, Error> {
        let sector_id_with_offset = sector_id + self.sector_offset;

        if sector_id >= 4096 {
            let cluster_id = ((sector_id - 4096) / 64) + 2;
            info!(
                "READ sector {} cluster {}",
                sector_id_with_offset, cluster_id
            );
        } else {
            info!("READ sector {}", sector_id_with_offset);
        }

        let pos = sector_id_with_offset as usize * BLOCK_SIZE;
        self.data_block
            .copy_from_slice(&self.image[pos..pos + BLOCK_SIZE]);

        Ok(&mut self.data_block)
    }

    async fn write_sector(&mut self, _sector_id: u32, _block: &Block) -> Result<(), Error> {
        todo!()
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
        }
    }
}

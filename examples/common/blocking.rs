use super::{EXAMPLE_EXFAT_IMAGE, print};
use exfat_slim::blocking::io::{BLOCK_SIZE, Block, BlockDevice, IoError};
use flate2::read::GzDecoder;
use std::fs;
use std::io::Read;

pub struct InMemoryBlockDevice {
    pub image: Vec<u8>,
    pub sector_offset: u32,
    pub data_block: [u8; BLOCK_SIZE],
    last_sector: Option<u32>,
}

impl InMemoryBlockDevice {
    pub fn new() -> Self {
        // this sd card image is 10MB uncompressed
        let buf = fs::read(EXAMPLE_EXFAT_IMAGE).unwrap();
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

impl BlockDevice for InMemoryBlockDevice {
    fn read_sector(&mut self, sector_id: u32, block: &mut Block) -> Result<(), IoError> {
        let sector_id_with_offset = sector_id + self.sector_offset;
        match self.last_sector.as_ref() {
            Some(x) if *x == sector_id_with_offset => {
                print("READ", sector_id_with_offset, sector_id, true)
            }
            _ => {
                print("READ", sector_id_with_offset, sector_id, false);
                self.last_sector = Some(sector_id_with_offset);
            }
        }

        let pos = sector_id_with_offset as usize * BLOCK_SIZE;
        block.copy_from_slice(&self.image[pos..pos + BLOCK_SIZE]);

        Ok(())
    }

    fn write_sector(&mut self, sector_id: u32, block: &Block) -> Result<(), IoError> {
        let sector_id_with_offset = sector_id + self.sector_offset;
        print("WRITE", sector_id_with_offset, sector_id, false);
        let pos = sector_id_with_offset as usize * BLOCK_SIZE;
        self.image[pos..pos + BLOCK_SIZE].copy_from_slice(block);
        self.last_sector = None;
        Ok(())
    }
}

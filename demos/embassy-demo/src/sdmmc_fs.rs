extern crate alloc;

use core::{fmt::Debug, ptr::addr_of_mut};

use defmt::info;
use embassy_stm32::{
    sdmmc::{
        self, Sdmmc,
        sd::{Addressable, Card, CmdBlock, DataBlock, StorageDevice},
    },
    time::mhz,
};
use exfat_slim::asynchronous::{
    fs::{self, BlockDeviceError},
    io::{BLOCK_SIZE, Block, BlockDevice},
};
use mbr_nostd::{MasterBootRecord, PartitionTable};

use crate::memory::{SD_CMD_BLOCK, SD_DATA_BLOCK};

#[allow(static_mut_refs)]
pub async fn file_system_task(mut sdmmc: Sdmmc<'static>) {
    let cmd_block = unsafe {
        let ptr = SD_CMD_BLOCK.as_mut_ptr();
        addr_of_mut!((*ptr)).write(CmdBlock([0u32; 16]));
        SD_CMD_BLOCK.assume_init_mut()
    };

    let data_block = unsafe {
        let ptr = SD_DATA_BLOCK.as_mut_ptr();
        addr_of_mut!((*ptr)).write(DataBlock([0u32; 128]));
        SD_DATA_BLOCK.assume_init_mut()
    };

    let storage = StorageDevice::new_uninit_sd_card(&mut sdmmc);

    let block_device = SdBlockDevice {
        cmd_block,
        data_block,
        storage,
        offset: 0,
        is_init: false,
    };

    fs::fs_actor_task(block_device).await;
}

struct SdBlockDevice<'a> {
    cmd_block: &'static mut CmdBlock,
    data_block: &'static mut DataBlock,
    storage: StorageDevice<'a, 'static, Card>,
    pub offset: u32,
    is_init: bool,
}

impl<'a> SdBlockDevice<'a> {
    pub async fn init(&mut self) -> Result<(), Error> {
        if !self.is_init {
            self.storage.reacquire(&mut self.cmd_block, mhz(25)).await?;
            let card = self.storage.card();
            info!("SD Card: {} bytes", card.size());
            self.offset = self.read_mbr().await?;
            self.is_init = true;
        }

        Ok(())
    }

    /// read the master boot record of the sd card to find the offset to the exfat boot sector
    async fn read_mbr(&mut self) -> Result<u32, Error> {
        let mut block = [0u8; BLOCK_SIZE];

        self.storage
            .read_block(0, &mut self.data_block)
            .await
            .map_err(Error::Io)?;
        block.copy_from_slice(self.data_block.as_slice());

        let mbr = MasterBootRecord::from_bytes(&block).unwrap();
        let entries = mbr.partition_table_entries();
        let num_entries = entries.len();
        let entry = entries.get(0).unwrap();
        let offset = entry.logical_block_address;

        info!(
            "reading MBR - offset: {} partition_type: {} sector_count: {}, num_entrties: {}",
            offset,
            entry.partition_type.to_mbr_tag_byte(),
            entry.sector_count,
            num_entries
        );
        Ok(offset)
    }
}

impl BlockDeviceError for Error {
    fn to_error_code(&self) -> u8 {
        match self {
            Error::Io(sdmmc::Error::BadClock) => 1,
            Error::Io(sdmmc::Error::Crc) => 2,
            Error::Io(sdmmc::Error::NoCard) => 3,
            Error::Io(sdmmc::Error::SignalingSwitchFailed) => 4,
            Error::Io(sdmmc::Error::SoftwareTimeout) => 5,
            Error::Io(sdmmc::Error::Timeout) => 6,
            Error::Io(sdmmc::Error::UnsupportedCardType) => 7,
            Error::Io(sdmmc::Error::UnsupportedCardVersion) => 8,
            _ => 0,
        }
    }

    fn no_card(&self) -> bool {
        matches!(
            self,
            Error::Io(sdmmc::Error::NoCard) | Error::Io(sdmmc::Error::Timeout)
        )
    }
}

impl<'a> Debug for SdBlockDevice<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SdBlockDevice")
            .field("sdmmc", &"SDMMC1")
            .finish()
    }
}

#[derive(Debug, defmt::Format)]
pub enum Error {
    Io(sdmmc::Error),
    Exfat(fs::Error),
}

impl From<fs::Error> for Error {
    fn from(value: fs::Error) -> Self {
        Self::Exfat(value)
    }
}

impl From<sdmmc::Error> for Error {
    fn from(value: sdmmc::Error) -> Self {
        Self::Io(value)
    }
}

impl<'a> BlockDevice for SdBlockDevice<'a> {
    type Error = Error;

    async fn read(&mut self, lba: u32, block: &mut Block) -> Result<(), Self::Error> {
        self.init().await?;
        let block_idx = self.offset + lba;
        match self
            .storage
            .read_block(block_idx, &mut self.data_block)
            .await
        {
            Ok(()) => {}
            Err(e) => {
                if e == sdmmc::Error::NoCard || e == sdmmc::Error::Timeout {
                    self.is_init = false;
                }
                return Err(Error::Io(e));
            }
        }

        block.copy_from_slice(self.data_block.as_slice());
        Ok(())
    }

    async fn write(&mut self, lba: u32, block: &Block) -> Result<(), Self::Error> {
        self.init().await?;
        let block_idx = self.offset + lba;
        assert_ne!(block_idx, 0); // you really don't want to write to this block on your sd card
        self.data_block.copy_from_slice(block.as_slice());

        match self.storage.write_block(block_idx, &self.data_block).await {
            Ok(()) => {}
            Err(e) => {
                if e == sdmmc::Error::NoCard || e == sdmmc::Error::Timeout {
                    self.is_init = false;
                }
                return Err(Error::Io(e));
            }
        }

        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

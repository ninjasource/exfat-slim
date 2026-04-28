extern crate alloc;

use core::fmt::Debug;

use aligned::{A4, Aligned};
use defmt::info;
use embassy_stm32::{
    sdmmc::{
        self, Sdmmc,
        sd::{Addressable, Card, CmdBlock, DataBlock, StorageDevice},
    },
    time::mhz,
};
use exfat_slim::asynchronous::{
    BlockDevice,
    fs::{self, BlockDeviceError},
};
use mbr_nostd::{MasterBootRecord, PartitionTable};

const BLOCK_SIZE: usize = 512;

pub async fn file_system_task(mut sdmmc: Sdmmc<'static>) {
    let storage = StorageDevice::new_uninit_sd_card(&mut sdmmc);

    let block_device = SdBlockDevice {
        storage,
        offset: 0,
        is_init: false,
    };

    fs::fs_actor_task::<_, _, 4>(block_device).await;
}

struct SdBlockDevice<'a> {
    storage: StorageDevice<'a, 'static, Card>,
    pub offset: u32,
    is_init: bool,
}

impl<'a> SdBlockDevice<'a> {
    pub async fn init(&mut self) -> Result<(), Error> {
        if !self.is_init {
            let mut cmd_block = CmdBlock::new();
            self.storage.reacquire(&mut cmd_block, mhz(25)).await?;
            let card = self.storage.card();
            info!("SD Card: {} bytes", card.size());
            self.offset = self.read_mbr().await?;
            self.is_init = true;
        }

        Ok(())
    }

    /// read the master boot record of the sd card to find the offset to the exfat boot sector
    async fn read_mbr(&mut self) -> Result<u32, Error> {
        let mut data_block = DataBlock::new();

        self.storage
            .read_block(0, &mut data_block)
            .await
            .map_err(Error::Io)?;

        let mbr = MasterBootRecord::from_bytes(&*data_block).unwrap();
        let entries = mbr.partition_table_entries();
        let num_entries = entries.len();
        let entry = entries.get(0).unwrap();
        let offset = entry.logical_block_address;

        info!(
            "reading master boot record - offset: {} partition_type: {} sector_count: {}, num_entrties: {}",
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

impl<'a> BlockDevice<BLOCK_SIZE> for SdBlockDevice<'a> {
    type Error = Error;
    type Align = A4;

    async fn read(
        &mut self,
        lba: u32,
        block: &mut [Aligned<Self::Align, [u8; BLOCK_SIZE]>],
    ) -> Result<(), Self::Error> {
        self.init().await?;
        let block_idx = self.offset + lba;
        match self.storage.read(block_idx, block).await {
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

    async fn write(
        &mut self,
        lba: u32,
        block: &[Aligned<Self::Align, [u8; BLOCK_SIZE]>],
    ) -> Result<(), Self::Error> {
        self.init().await?;
        let block_idx = self.offset + lba;
        assert_ne!(block_idx, 0); // you really don't want to write to this block on your sd card

        match self.storage.write(block_idx, block).await {
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

    async fn size(&mut self) -> Result<u64, Self::Error> {
        Ok(self.storage.size().await?)
    }
}

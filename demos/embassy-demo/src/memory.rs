use core::mem::MaybeUninit;

use embassy_stm32::sdmmc::sd::{CmdBlock, DataBlock};

use crate::backup_ram::BackupRam;

// sdmmc dma
pub static mut SD_CMD_BLOCK: MaybeUninit<CmdBlock> = MaybeUninit::uninit();

// sdmmc dma
pub static mut SD_DATA_BLOCK: MaybeUninit<DataBlock> = MaybeUninit::uninit();

// 1KB of battery backed ram (the other 1KB is used for backup logs)
#[unsafe(link_section = ".backup_ram")]
#[used]
pub static mut BACKUP_RAM: BackupRam = BackupRam::zeroed();

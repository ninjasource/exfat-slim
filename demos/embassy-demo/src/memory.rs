use core::mem::MaybeUninit;
use embassy_stm32::sdmmc::sd::{CmdBlock, DataBlock};

// sdmmc dma
pub static mut SD_CMD_BLOCK: MaybeUninit<CmdBlock> = MaybeUninit::uninit();

// sdmmc dma
pub static mut SD_DATA_BLOCK: MaybeUninit<DataBlock> = MaybeUninit::uninit();

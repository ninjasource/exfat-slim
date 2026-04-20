#![no_std]

use embedded_alloc::Heap;
pub mod backup_ram;
pub mod memory;
pub mod sdmmc_fs;
pub mod time;

extern crate alloc;

#[global_allocator]
pub static ALLOCATOR: Heap = Heap::empty();

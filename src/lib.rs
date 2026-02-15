#![allow(clippy::duplicate_mod)]
#![deny(unsafe_code)]
#![no_std]
extern crate alloc;

#[path = "."]
pub mod blocking {
    pub use bisync::synchronous::*;

    pub mod allocation_bitmap;
    pub mod boot_sector;
    pub mod directory_entry;
    pub mod error;
    pub mod fat;
    pub mod file;
    pub mod file_system;
    pub mod io;
    pub mod mocks;
    pub mod upcase_table;
    pub mod utils;
}

#[path = "."]
pub mod asynchronous {
    pub use bisync::asynchronous::*;

    pub mod allocation_bitmap;
    pub mod boot_sector;
    pub mod directory_entry;
    pub mod error;
    pub mod fat;
    pub mod file;
    pub mod file_system;
    pub mod io;
    pub mod mocks;
    pub mod upcase_table;
    pub mod utils;
}

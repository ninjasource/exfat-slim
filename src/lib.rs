#![allow(clippy::duplicate_mod)]
#![deny(unsafe_code)]
#![no_std]

extern crate alloc;

#[path = "."]
pub mod blocking {
    pub use bisync::synchronous::*;

    mod allocation_bitmap;
    pub mod boot_sector;
    pub mod directory_entry;
    pub mod error;
    mod fat;
    pub mod file;
    pub mod file_system;
    pub mod io;
    mod upcase_table;
    pub mod utils;

    //  #[cfg(test)]
    //  mod mocks;
}

#[path = "."]
pub mod asynchronous {
    pub use bisync::asynchronous::*;

    mod allocation_bitmap;
    pub mod boot_sector;
    pub mod directory_entry;
    pub mod error;
    mod fat;
    pub mod file;
    pub mod file_system;
    pub mod io;

    mod upcase_table;
    pub mod utils;

    //  #[cfg(test)]
    //  mod mocks;
}

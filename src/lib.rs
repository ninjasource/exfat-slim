#![allow(unused_variables, unused_imports)]
#![deny(unsafe_code)]
#![no_std]
extern crate alloc;

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

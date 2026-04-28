#![allow(clippy::duplicate_mod)]
#![deny(unsafe_code)]
#![no_std]

extern crate alloc;

#[path = "."]
pub mod blocking {
    pub use bisync::synchronous::*;

    mod block_device;
    pub use block_device::BlockDevice;

    mod allocation;
    pub mod boot_sector;
    pub mod directory;
    mod directory_entry;
    pub mod error;
    mod fat;
    pub mod file;
    pub mod file_system;
    mod slot_cache;
    mod upcase_table;
    pub mod utils;

    //#[cfg(test)]
    //mod mocks;
}

#[path = "."]
pub mod asynchronous {
    pub use bisync::asynchronous::*;
    mod allocation;
    pub mod boot_sector;
    pub mod directory;
    mod directory_entry;
    pub mod error;
    mod fat;
    pub mod file;
    pub mod file_system;
    #[cfg(feature = "embassy")]
    pub mod fs;
    mod slot_cache;
    mod upcase_table;
    pub mod utils;

    pub use block_device_driver::BlockDevice;

    //#[cfg(test)]
    //mod mocks;
}

#[cfg(all(feature = "log", feature = "defmt"))]
compile_error!("features `log` and `defmt` are mutually exclusive");

#[cfg(feature = "defmt")]
pub use defmt::{debug, error, info, trace, warn};

#[cfg(all(feature = "log", not(feature = "defmt")))]
pub use log::{debug, error, info, trace, warn};

#[cfg(not(any(feature = "log", feature = "defmt")))]
mod no_log {
    #[macro_export]
    macro_rules! error {
        ($($tt:tt)*) => {};
    }
    #[macro_export]
    macro_rules! warn {
        ($($tt:tt)*) => {};
    }
    #[macro_export]
    macro_rules! info {
        ($($tt:tt)*) => {};
    }
    #[macro_export]
    macro_rules! debug {
        ($($tt:tt)*) => {};
    }
    #[macro_export]
    macro_rules! trace {
        ($($tt:tt)*) => {};
    }
}

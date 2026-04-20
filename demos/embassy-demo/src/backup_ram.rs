use core::{
    ffi::CStr,
    ptr::{addr_of, addr_of_mut},
};

use alloc::string::String;
use embassy_stm32::pac;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

use crate::memory::BACKUP_RAM;

// NOTE: do not use fancy data types like Vec and String because the memory could be anything which would cause UB
#[repr(C)]
#[derive(Debug, defmt::Format)]
pub struct BackupRam {
    pub magic: u32,
    pub version: u32,
    // this is saved as a cstring (null terminated)
    pub file_name: [u8; 20],
    pub daily_reset_counter: u32,
    pub daily_log_bytes: u64,
}

impl BackupRam {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            file_name: [0; 20],
            daily_reset_counter: 0,
            daily_log_bytes: 0,
        }
    }

    pub fn try_get_file_name(&self) -> Option<String> {
        match CStr::from_bytes_until_nul(&self.file_name) {
            Ok(cstr) => match cstr.to_str() {
                Ok(s) => {
                    if s.len() == 0 {
                        None
                    } else {
                        Some(s.into())
                    }
                }
                Err(_e) => None,
            },
            Err(_e) => None,
        }
    }

    pub fn set_file_name(&mut self, file_name: Option<String>) {
        match file_name {
            Some(file_name) => {
                let bytes = file_name.as_bytes();
                self.file_name[..bytes.len()].copy_from_slice(bytes);
                self.file_name[bytes.len()] = b'\0';
            }
            None => {
                self.file_name[0] = b'\0';
            }
        }
    }
}

// Mutex metadata lives in normal RAM. It only protects access.
static BACKUP_RAM_LOCK: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

pub struct BackupRamHandle;

pub static BACKUP: BackupRamHandle = BackupRamHandle;

impl BackupRamHandle {
    pub fn read<R>(&self, f: impl FnOnce(&BackupRam) -> R) -> R {
        BACKUP_RAM_LOCK.lock(|_| {
            let br = unsafe { &*addr_of!(BACKUP_RAM) };
            f(br)
        })
    }

    pub fn write<R>(&self, f: impl FnOnce(&mut BackupRam) -> R) -> R {
        BACKUP_RAM_LOCK.lock(|_| {
            let br = unsafe { &mut *addr_of_mut!(BACKUP_RAM) };
            f(br)
        })
    }

    pub fn init_if_needed(&self) {
        self.write(|br| {
            const MAGIC: u32 = 0x4252_414D; // "BRAM"
            const VERSION: u32 = 1;

            if br.magic != MAGIC || br.version != VERSION {
                *br = BackupRam::zeroed();
                br.magic = MAGIC;
                br.version = VERSION;
            }
        });
    }
}

pub fn enable_backup_memory_writes() {
    pac::RCC.ahb3enr().modify(|w| w.set_pwren(true));
    pac::PWR.bdcr1().modify(|w| w.set_bren(true));
    pac::PWR.dbpcr().modify(|w| w.set_dbp(true));
    pac::RCC.ahb1enr().modify(|w| w.set_bkpsramen(true));
}

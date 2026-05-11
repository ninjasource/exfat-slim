use core::ptr::{addr_of, addr_of_mut};

extern crate alloc;

use embassy_stm32::pac;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

// 1KB of battery backed ram (the other 1KB is used for backup logs)
#[unsafe(link_section = ".backup_ram")]
#[used]
static mut BACKUP_RAM: BackupRam = BackupRam::zeroed();

// Mutex metadata lives in normal RAM. It only protects access.
static BACKUP_RAM_LOCK: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

// NOTE: do not use fancy data types like Vec and String because the memory could be anything which would cause UB
#[repr(C)]
#[derive(Debug, defmt::Format)]
pub struct BackupRam {
    pub magic: u32,
    pub version: u32,
    pub daily_reset_counter: u32,
    pub daily_log_bytes: u64,
}

impl BackupRam {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            daily_reset_counter: 0,
            daily_log_bytes: 0,
        }
    }
}

pub fn read<R>(f: impl FnOnce(&BackupRam) -> R) -> R {
    BACKUP_RAM_LOCK.lock(|_| {
        // SAFETY: exclusive access guaranteed by lock and contains primitives so always valid
        let br = unsafe { &*addr_of!(BACKUP_RAM) };
        f(br)
    })
}

pub fn write(f: impl FnOnce(&mut BackupRam)) {
    BACKUP_RAM_LOCK.lock(|_| {
        // SAFETY: exclusive access guaranteed by lock and contains primitives so always valid
        let br = unsafe { &mut *addr_of_mut!(BACKUP_RAM) };
        f(br)
    })
}

pub fn init_if_needed() {
    write(|br| {
        const MAGIC: u32 = 0x4252_414D; // "BRAM"
        const VERSION: u32 = 1;

        if br.magic != MAGIC || br.version != VERSION {
            *br = BackupRam::zeroed();
            br.magic = MAGIC;
            br.version = VERSION;
        }
    });
}

pub fn enable_backup_memory_writes() {
    pac::RCC.ahb3enr().modify(|w| w.set_pwren(true));
    pac::PWR.bdcr1().modify(|w| w.set_bren(true));
    pac::PWR.dbpcr().modify(|w| w.set_dbp(true));
    pac::RCC.ahb1enr().modify(|w| w.set_bkpsramen(true));
}

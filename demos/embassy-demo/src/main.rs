// This demonstrates an example use case saving logs to an SD card formatted to the exFAT file system

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

extern crate alloc;

pub mod backup_ram;
pub mod logger_fs;
pub mod memory;
pub mod rcc_setup;
pub mod sdmmc_fs;
pub mod time;

#[global_allocator]
pub static ALLOCATOR: embedded_alloc::Heap = embedded_alloc::Heap::empty();

use chrono::Timelike;
use defmt::{error, info, unwrap};
use defmt_persist::{self as _, ConsumerAndMetadata};
use embassy_demo::error::Error;
use embassy_executor::Spawner;
use embassy_stm32::{
    bind_interrupts,
    gpio::{Input, Pull},
    peripherals::{self},
    rtc::{DateTime, DayOfWeek, Rtc, RtcConfig},
    sdmmc::{self, Sdmmc},
};
use embassy_time::{Duration, Timer};
use exfat_slim::asynchronous::{file::OpenOptions, fs};

use crate::{
    logger_fs::{flush_logs, logger_loop},
    time::rtc_unix_ms_now,
};

bind_interrupts!(struct Irqs {
    SDMMC1 => sdmmc::InterruptHandler<peripherals::SDMMC1>;
});

const HEAP_SIZE: usize = 1_500_000;
pub static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", defmt::Display2Format(info));
    cortex_m::peripheral::SCB::sys_reset(); // Or hardfault if it should go via fault handlers.
}

#[embassy_executor::task()]
async fn file_system_task(sdmmc: Sdmmc<'static>) {
    sdmmc_fs::file_system_task(sdmmc).await;
    info!("file_system_task ended");
}

#[embassy_executor::task()]
async fn logger_task(mut logger: ConsumerAndMetadata<'static>, mut sd_detect: Input<'static>) {
    let mut error = None;
    loop {
        // the logger loop will exit if there is no sd card but at
        // some point the user can put a card in and the system will recover
        match logger_loop(&mut logger, &mut sd_detect).await {
            Ok(()) => {}
            Err(e) => {
                // we cannot log the error because it will compound the issue
                // the system will continually fail to write the error it just attempted to log in a never ending loop
                // a compromise is to log the first occurance of a persistence error and never again
                if error.is_none() {
                    error = Some(e);
                    error!("log persistence error: {:?}", error.as_ref().unwrap())
                }
            }
        }

        Timer::after_secs(1).await;
    }
}

const RTC_MAGIC_REG: usize = 0;
const RTC_MAGIC: u32 = 0x51A2_C3D4;

#[embassy_executor::main]
async fn main(mut spawner: Spawner) {
    let p = rcc_setup::stm32u5g9zj_29mhz_init();
    unsafe { ALLOCATOR.init(&HEAP as *const u8 as usize, core::mem::size_of_val(&HEAP)) }

    let (rtc, _time_provider) = Rtc::new(p.RTC, RtcConfig::default());
    let sd_detect = Input::new(p.PD4, Pull::Up); // low - card inserted,
    setup_logging(&mut spawner, rtc, sd_detect);

    // sd card setup
    let sdmmc = Sdmmc::new_4bit(
        p.SDMMC1,
        Irqs,
        p.PC12,
        p.PD2,
        p.PC8,
        p.PC9,
        p.PC10,
        p.PC11,
        Default::default(),
    );
    spawner.spawn(unwrap!(file_system_task(sdmmc)));

    // housekeeping
    spawner.spawn(unwrap!(daily_reset_task()));
    Timer::after_millis(100).await;

    match do_stuff().await {
        Ok(()) => {}
        Err(e) => {
            error!("{:?}", e);
        }
    }

    let mut i = 0;
    loop {
        Timer::after(Duration::from_millis(1000)).await;
        info!("tick {}", i);
        i += 1;
    }
}

async fn do_stuff() -> Result<(), Error> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open("bla.bin")
        .await?;
    info!("opened file");

    let block = [0xAB; 512];
    let count = 2048;

    for i in 0..count {
        file.write(&block).await.unwrap();

        if i % 32 == 0 && i > 0 {
            info!("wrote 16KB");
        }
    }
    file.close().await?;
    info!("file closed");
    flush_logs().await;

    let mut i = 0;
    loop {
        Timer::after(Duration::from_millis(1000)).await;
        info!("blink {}", i);
        i += 1;
    }
}

fn setup_logging(spawner: &mut Spawner, mut rtc: Rtc, sd_detect: Input<'static>) {
    // Only initialize calendar once.
    let already_initialized = rtc.read_backup_register(RTC_MAGIC_REG) == Some(RTC_MAGIC);

    if !already_initialized {
        rtc.set_datetime(DateTime::from(2026, 3, 23, DayOfWeek::Monday, 2, 24, 0, 0).unwrap())
            .unwrap();
        rtc.write_backup_register(RTC_MAGIC_REG, RTC_MAGIC);
    }
    defmt::timestamp!("{=u64:iso8601ms}", rtc_unix_ms_now());

    let Ok(logger) = defmt_persist::init() else {
        panic!("log init failed");
    };

    spawner.spawn(unwrap!(logger_task(logger, sd_detect)));
    backup_ram::init_if_needed();
}

#[embassy_executor::task]
async fn daily_reset_task() -> ! {
    let mut start_ms = rtc_unix_ms_now();
    const _1HOUR_MS: u64 = 60 * 60 * 1000;

    loop {
        let unix_time_ms = rtc_unix_ms_now();

        // handle the case where the unit fetches the time from the controller
        // the unit starts time at 1 jan 1970 (unix timestamp 0)
        if (unix_time_ms - start_ms) > 24 * _1HOUR_MS {
            start_ms = unix_time_ms;
            continue;
        }

        // if more than 2 hours has elapsed
        // prevents repeated restarts if the time has not been fetched from the controller
        if (unix_time_ms - start_ms) > 2 * _1HOUR_MS {
            let now = chrono::DateTime::from_timestamp_millis(unix_time_ms as i64).unwrap();
            if now.time().hour() == 1 {
                let mut daily_log_bytes = 0;
                let mut daily_reset_counter = 0;

                backup_ram::write(|x| {
                    daily_log_bytes = x.daily_log_bytes;
                    daily_reset_counter = x.daily_reset_counter;
                    x.daily_log_bytes = 0;
                    x.daily_reset_counter = 0;
                });

                info!("Time is 1am, restart is scheduled");
                info!(
                    "log bytes: {} reset count: {}",
                    daily_log_bytes, daily_reset_counter
                );

                flush_logs().await;
                fs::shutdown().await.ok();
                Timer::after_millis(100).await;
                on_system_restart();
            }
        }

        Timer::after_secs(1).await;
    }
}

pub fn on_system_restart() {
    info!("system rebooting");
    defmt::flush();
    cortex_m::peripheral::SCB::sys_reset();
}

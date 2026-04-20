// This demonstrates an example use case saving logs to an SD card formatted to the exFAT file system

#![no_std]
#![no_main]
#![macro_use]
#![allow(static_mut_refs)]

extern crate alloc;

use chrono::Timelike;
use defmt::{error, info, unwrap};
use defmt_persist::{self as _, Consumer, ConsumerAndMetadata};
use embassy_demo::{ALLOCATOR, backup_ram::BACKUP, rcc_setup, sdmmc_fs, time::rtc_unix_ms_now};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_stm32::{
    bind_interrupts,
    gpio::{Input, Pull},
    peripherals::{self},
    rtc::{DateTime, DayOfWeek, Rtc, RtcConfig},
    sdmmc::{self, Sdmmc},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use exfat_slim::asynchronous::{
    file::OpenOptions,
    fs::{self, FileHandle},
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

#[derive(Debug, defmt::Format)]
enum Error {
    Fs(fs::Error),
}

impl From<fs::Error> for Error {
    fn from(value: fs::Error) -> Self {
        Self::Fs(value)
    }
}

#[embassy_executor::task()]
async fn file_system_task(sdmmc: Sdmmc<'static>) {
    sdmmc_fs::file_system_task(sdmmc).await;
    info!("file_system_task ended");
}

#[embassy_executor::task()]
async fn logger_task(mut log_init: ConsumerAndMetadata<'static>, mut sd_detect: Input<'static>) {
    loop {
        // the logger loop will exit if there is no sd card but at
        // some point the user can put a card in and the system will recover
        match logger_loop(&mut log_init, &mut sd_detect).await {
            Ok(()) => {}
            Err(_e) => {} // error!("{:?}", _e),
        }

        Timer::after_secs(1).await;
    }
}

const MAX_LOG_BYTES_PER_DAY: u64 = 1024 * 1024; // 1MB
static FLUSH_LOGS: Channel<CriticalSectionRawMutex, (), 1> = Channel::new();

async fn logger_loop(
    log: &mut ConsumerAndMetadata<'static>,
    sd_detect: &mut Input<'static>,
) -> Result<(), Error> {
    if sd_detect.is_high() {
        return Err(Error::Fs(fs::Error::NoCard));
    }

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open("log.bin")
        .await?;

    loop {
        while !log.consumer.is_empty() {
            write_to_file(&mut log.consumer, &file).await?;
        }

        match select(log.consumer.wait_for_data(), FLUSH_LOGS.receive()).await {
            Either::First(_) => {
                if !log.consumer.is_empty() {
                    write_to_file(&mut log.consumer, &file).await?;
                }
            }
            Either::Second(_) => {
                file.flush().await?;
                info!("log flushed");
            }
        }
    }
}

async fn write_to_file(consumer: &mut Consumer<'_>, file: &FileHandle) -> Result<(), Error> {
    let grant = consumer.read();
    let (a, b) = grant.bufs();
    let num_bytes = a.len() + b.len();

    if !daily_limit_exceeded(num_bytes) {
        file.write(a).await?;
        file.write(b).await?;
    }

    grant.release_all();
    Ok(())
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
}

fn daily_limit_exceeded(num_bytes: usize) -> bool {
    let mut daily_log_bytes = 0;
    BACKUP.write(|x| {
        daily_log_bytes = x.daily_log_bytes;
        x.daily_log_bytes.wrapping_add(num_bytes as u64)
    });

    daily_log_bytes > MAX_LOG_BYTES_PER_DAY
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

async fn flush_logs() {
    FLUSH_LOGS.send(()).await;
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

    let Ok(log_init) = defmt_persist::init() else {
        panic!("log init failed");
    };

    spawner.spawn(unwrap!(logger_task(log_init, sd_detect)));
    BACKUP.init_if_needed();
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

                BACKUP.write(|x| {
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

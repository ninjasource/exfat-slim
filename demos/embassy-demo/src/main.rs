#![no_std]
#![no_main]
#![macro_use]
#![allow(static_mut_refs)]
#![allow(dead_code)]

extern crate alloc;

use alloc::vec;
use chrono::Timelike;
use defmt::{error, info, unwrap};
use defmt_persist::{self as _, ConsumerAndMetadata};
use embassy_demo::{ALLOCATOR, backup_ram::BACKUP, sdmmc_fs, time::rtc_unix_ms_now};
use embassy_executor::{SpawnError, Spawner};
use embassy_stm32::{
    Config, bind_interrupts,
    gpio::{Input, Pull},
    peripherals::{self},
    rtc::{DateTime, DayOfWeek, Rtc, RtcConfig},
    sdmmc::{self, Sdmmc},
};
use embassy_time::{Duration, Timer};
use exfat_slim::asynchronous::{file::OpenOptions, fs, io::BLOCK_SIZE};

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
    Task(SpawnError),
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
        logger_loop(&mut log_init, &mut sd_detect).await.ok();
        Timer::after_secs(1).await;
    }
}

const MAX_LOG_BYTES_PER_DAY: u64 = 1024 * 1024;

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

    let meta = file.metdata().await?;
    info!("file len {}", meta.len());

    let mut scratch = vec![0u8; BLOCK_SIZE];

    if let Ok(metadata) = file.metdata().await {
        let mut file_len = metadata.len();
        loop {
            log.consumer.wait_for_data().await;
            let grant = log.consumer.read();
            let (buf0, buf1) = grant.bufs();
            let num_bytes = buf0.len() + buf1.len();

            // we want this to be BLOCK_SIZE but it may be smaller if the file len is not alligned
            // this can happen if logs are force purged
            let to_write = BLOCK_SIZE - (file_len as usize % BLOCK_SIZE);

            let bytes_written = if num_bytes >= to_write {
                let mut daily_log_bytes = 0;
                BACKUP.write(|x| {
                    daily_log_bytes = x.daily_log_bytes;
                    x.daily_log_bytes.wrapping_add(num_bytes as u64)
                });

                // limit to size of log growth per day
                if daily_log_bytes < MAX_LOG_BYTES_PER_DAY {
                    if buf0.len() >= to_write {
                        // we can get everything from buf0
                        file.write(&buf0[..to_write]).await?;
                    } else {
                        copy_to_scratch(&mut scratch, buf0, buf1, to_write);
                        file.write(&scratch[..to_write]).await?;
                    }
                }

                file_len += to_write as u64;
                to_write
            } else {
                // don't bother writing to disk because it does not fall on a block boundary
                0
            };

            grant.release(bytes_written);

            if bytes_written == 0 {
                // if we don't release the bytes then the consumer.wait_for_data() will continue to fire
                // I think that this is a bug because it should only fire if there is NEW data.
                // The sleep below prevents executor starvation
                // I have tried to handle SD card removal here but it just seems to lock the processor up
                // for a few seconds every time it gets done
                Timer::after_secs(1).await;
            }
        }
    }

    Ok(())
}

fn copy_to_scratch(scratch: &mut [u8], buf0: &[u8], buf1: &[u8], len: usize) {
    scratch[..buf0.len()].copy_from_slice(buf0);
    scratch[buf0.len()..len].copy_from_slice(&buf1[..len - buf0.len()]);
}

const RTC_MAGIC_REG: usize = 0;
const RTC_MAGIC: u32 = 0x51A2_C3D4;

#[embassy_executor::main]
async fn main(mut spawner: Spawner) {
    let config = Config::default();
    let p = embassy_stm32::init(config);
    //let p = rcc_setup::stm32u5g9zj_29mhz_init();
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

    match do_stuff().await {
        Ok(()) => {}
        Err(e) => {
            error!("{:?}", e);
        }
    }
}

async fn do_stuff() -> Result<(), Error> {
    // TODO: figure out why this doesn't work
    /*
    info!("writing file");
    fs::write("hello.txt", b"Hello, world!").await?;
    let text = fs::read_to_string("hello.txt").await?;
    info!("hello.txt: {}", text);
    */

    info!("files in root:");
    let dir = fs::read_dir("").await?;
    while let Some(entry) = dir.next_entry().await? {
        info!("{}", entry.file_name());
    }

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

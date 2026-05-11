use defmt::info;
use defmt_persist::ConsumerAndMetadata;
use embassy_demo::{error::Error, helpers::LoggerHelper};
use embassy_futures::select::{Either, select};
use embassy_stm32::gpio::Input;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use exfat_slim::asynchronous::{
    file::OpenOptions,
    fs::{self, FileHandle},
};

use crate::backup_ram::{self};

const MAX_LOG_BYTES_PER_DAY: u64 = 1024 * 1024; // 1MB
static FLUSH_LOGS: Channel<CriticalSectionRawMutex, (), 1> = Channel::new();

pub async fn logger_loop(
    logger: &mut ConsumerAndMetadata<'static>,
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

    let file_len = file.metdata().await?.len();
    let mut helper = LoggerHelper::new(file_len);

    loop {
        write_to_file(logger, &mut helper, false, &file).await?;

        match select(logger.consumer.wait_for_data(), FLUSH_LOGS.receive()).await {
            Either::First(_) => {
                write_to_file(logger, &mut helper, false, &file).await?;
            }
            Either::Second(_) => {
                write_to_file(logger, &mut helper, true, &file).await?;
                info!("log flushed");
            }
        }
    }
}

pub async fn flush_logs() {
    FLUSH_LOGS.send(()).await;
}

async fn write_to_file(
    logger: &mut ConsumerAndMetadata<'static>,
    helper: &mut LoggerHelper,
    force_flush: bool,
    file: &FileHandle,
) -> Result<(), Error> {
    if logger.consumer.is_empty() {
        if force_flush {
            file.flush().await?;
        }

        return Ok(());
    }

    let grant = logger.consumer.read();
    let (a, b) = grant.bufs();
    let len_a = a.len();
    let len_b = b.len();
    let num_bytes = len_a + len_b;

    if daily_limit_exceeded(num_bytes) {
        // swallow the logs and don't write them to disk
        grant.release_all();
    } else {
        if force_flush {
            helper.update_remainder(len_a, len_b);
            file.write(a).await?;
            file.write(b).await?;
            file.flush().await?;
            grant.release_all();
        } else {
            if let Some((write_a, write_b)) = helper.to_write(len_a, len_b) {
                // we have enough bytes to write a complete sector to disk
                file.write(&a[..write_a]).await?;
                file.write(&b[..write_b]).await?;

                // we must flush we are releasing the grant with the bytes freed up
                // this is required fot the logs to be durable
                file.flush().await?;
                grant.release(write_a + write_b);
            }
        }
    }

    // it is perfectly acceptible to not log anything to disk
    // and build up several logs in the backup RAM instead

    Ok(())
}

fn daily_limit_exceeded(num_bytes: usize) -> bool {
    let mut daily_log_bytes = 0;
    backup_ram::write(|x| {
        daily_log_bytes = x.daily_log_bytes;
        x.daily_log_bytes = x.daily_log_bytes.wrapping_add(num_bytes as u64);
    });

    daily_log_bytes > MAX_LOG_BYTES_PER_DAY
}

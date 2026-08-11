use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use embassy_stm32::pac;
use exfat_slim::timestamp::Timestamp;

pub fn rtc_unix_ms_now() -> u64 {
    //sync_rtc_shadow_registers_once();

    let (year, month, day, hour, minute, second, ssr, prediv_s) =
        read_rtc_calendar_and_ssr_stable();

    let date = NaiveDate::from_ymd_opt(year, month, day).unwrap();
    let time = NaiveTime::from_hms_opt(hour, minute, second).unwrap();
    let dt = NaiveDateTime::new(date, time);

    let unix_s = dt.and_utc().timestamp();
    let sub_ms = (((prediv_s - ssr) as i64) * 1000) / ((prediv_s + 1) as i64);

    (unix_s * 1000 + sub_ms) as u64
}

pub fn read_rtc_calendar() -> (i32, u32, u32, u32, u32, u32) {
    let rtc = pac::RTC;

    // Read TR and DR. Avoid SSR here; seconds resolution is simpler and safer.
    let tr1 = rtc.tr().read().0;
    let dr1 = rtc.dr().read().0;
    let tr2 = rtc.tr().read().0;
    let dr2 = rtc.dr().read().0;

    // Basic stability check: if rollover happened during read, retry once.
    let (tr, dr) = if tr1 == tr2 && dr1 == dr2 {
        (tr1, dr1)
    } else {
        let tr = rtc.tr().read().0;
        let dr = rtc.dr().read().0;
        (tr, dr)
    };

    let second = bcd2bin(((tr >> 0) & 0x7f) as u8) as u32;
    let minute = bcd2bin(((tr >> 8) & 0x7f) as u8) as u32;
    let hour = bcd2bin(((tr >> 16) & 0x3f) as u8) as u32;

    let day = bcd2bin(((dr >> 0) & 0x3f) as u8) as u32;
    let month = bcd2bin(((dr >> 8) & 0x1f) as u8) as u32;
    let year = 2000 + bcd2bin(((dr >> 16) & 0xff) as u8) as i32;

    (year, month, day, hour, minute, second)
}

fn read_rtc_calendar_and_ssr_stable() -> (i32, u32, u32, u32, u32, u32, u32, u32) {
    let rtc = pac::RTC;

    // Read until two consecutive samples match.
    // This avoids mixed values across rollover.
    loop {
        let ssr1 = rtc.ssr().read().ss();
        let tr1 = rtc.tr().read().0;
        let dr1 = rtc.dr().read().0;

        let ssr2 = rtc.ssr().read().ss();
        let tr2 = rtc.tr().read().0;
        let dr2 = rtc.dr().read().0;

        if tr1 == tr2 && dr1 == dr2 && ssr1 == ssr2 {
            let second = bcd2bin(((tr1 >> 0) & 0x7f) as u8) as u32;
            let minute = bcd2bin(((tr1 >> 8) & 0x7f) as u8) as u32;
            let hour = bcd2bin(((tr1 >> 16) & 0x3f) as u8) as u32;

            let day = bcd2bin(((dr1 >> 0) & 0x3f) as u8) as u32;
            let month = bcd2bin(((dr1 >> 8) & 0x1f) as u8) as u32;
            let year = 2000 + bcd2bin(((dr1 >> 16) & 0xff) as u8) as i32;

            let prediv_s = 255;

            return (year, month, day, hour, minute, second, ssr1, prediv_s);
        }
    }
}

pub fn rtc_timestamp_now() -> Timestamp {
    let (year, month, day, hour, minute, second, ssr, prediv_s) =
        read_rtc_calendar_and_ssr_stable();
    let centis = ((prediv_s - ssr) as u32 * 100 / (prediv_s as u32 + 1)) as u8;
    let timestamp = Timestamp {
        year: year as u16,
        month: month as u8,
        day: day as u8,
        hour: hour as u8,
        minute: minute as u8,
        second: second as u8,
        centis,
        utc_offset_15min: Some(0), // UTC for now
    };

    if timestamp.is_valid() {
        timestamp
    } else {
        // the UTC might be out of range
        Timestamp::default()
    }
}

#[inline]
const fn bcd2bin(x: u8) -> u8 {
    (x & 0x0f) + ((x >> 4) * 10)
}

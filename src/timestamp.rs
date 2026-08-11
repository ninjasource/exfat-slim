const MIN_TIMESTAMP: u32 = 0x0021_0000;
const MIN_YEAR: u16 = 1980;
const MAX_YEAR: u16 = 2107;
const UTC_OFFSET_VALID: u8 = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    pub year: u16, // 1980..=2107
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub centis: u8,                   // centi seconds 0-99 (10ms increments)
    pub utc_offset_15min: Option<i8>, // None = offset unknown
}

impl Timestamp {
    pub(crate) fn encode(&self) -> EncodedTimestamp {
        // panic on debug builds, min timestamp on release
        debug_assert!(self.is_valid());
        if !self.is_valid() {
            return EncodedTimestamp::default();
        }

        let packed = ((self.year - MIN_YEAR) as u32) << 25
            | (self.month as u32) << 21
            | (self.day as u32) << 16
            | (self.hour as u32) << 11
            | (self.minute as u32) << 5
            | (self.second as u32) >> 1; // double-seconds, 0-29

        let increment_10ms = (self.second & 1) * 100 + self.centis;

        let utc_offset = match self.utc_offset_15min {
            Some(offset) => UTC_OFFSET_VALID | (offset as u8 & 0x7F),
            None => 0,
        };

        EncodedTimestamp {
            packed,
            increment_10ms,
            utc_offset,
        }
    }

    pub(crate) fn decode(encoded: EncodedTimestamp) -> Self {
        let packed = encoded.packed;
        let extra_secs = encoded.increment_10ms / 100; // 0 or 1
        let second = ((packed as u8 & 0x1F) << 1) + extra_secs;
        let centis = encoded.increment_10ms % 100;
        let utc_offset_15min = if encoded.utc_offset & UTC_OFFSET_VALID != 0 {
            Some(((encoded.utc_offset << 1) as i8) >> 1)
        } else {
            None
        };

        Self {
            year: (packed >> 25) as u16 + MIN_YEAR,
            month: (packed >> 21) as u8 & 0x0F,
            day: (packed >> 16) as u8 & 0x1F,
            hour: (packed >> 11) as u8 & 0x1F,
            minute: (packed >> 5) as u8 & 0x3F,
            second,
            centis,
            utc_offset_15min,
        }
    }

    pub fn is_valid(&self) -> bool {
        (MIN_YEAR..=MAX_YEAR).contains(&self.year)
            && (1..=days_in_month(self.year, self.month)).contains(&self.day)
            && self.hour <= 23
            && self.minute <= 59
            && self.second <= 59
            && self.centis <= 99
            && self
                .utc_offset_15min
                .is_none_or(|offset| (-48..=56).contains(&offset))
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Self::decode(EncodedTimestamp::default())
    }
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28 + is_leap_year(year) as u8,
        _ => 0,
    }
}

fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EncodedTimestamp {
    pub packed: u32,
    pub increment_10ms: u8,
    pub utc_offset: u8,
}

impl Default for EncodedTimestamp {
    fn default() -> Self {
        Self {
            packed: MIN_TIMESTAMP,
            increment_10ms: 0,
            utc_offset: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // test a difficult timestamp
    // - an odd second (triggering an increment carry),
    // - a non-sero centi second
    // - a negative utc offset (triggering a sign extension)
    #[test]
    fn round_trip() {
        let timestamp = Timestamp {
            year: 2026,
            month: 8,
            day: 11,
            hour: 14,
            minute: 30,
            second: 37,
            centis: 25,
            utc_offset_15min: Some(-20), // UTC-5
        };
        let encoded = timestamp.encode();
        assert_eq!(encoded.increment_10ms, 125);
        assert_eq!(encoded.utc_offset, 0xEC);
        assert_eq!(Timestamp::decode(encoded), timestamp);
    }

    // test that the min allowed timestamp in exfat is the default
    #[test]
    fn min_timestamp_is_epoch() {
        let actual = Timestamp::decode(EncodedTimestamp::default());
        let expected = Timestamp {
            year: 1980,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            centis: 0,
            utc_offset_15min: None,
        };
        assert_eq!(actual, expected);
        assert_eq!(actual.encode(), EncodedTimestamp::default());
    }

    #[test]
    fn validity_bounds() {
        let ok = Timestamp {
            year: 2107,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            centis: 99,
            utc_offset_15min: Some(56),
        };
        assert!(ok.is_valid());
        assert!(!Timestamp { year: 1979, ..ok }.is_valid());
        assert!(!Timestamp { year: 2108, ..ok }.is_valid());
        assert!(
            !Timestamp {
                utc_offset_15min: Some(57),
                ..ok
            }
            .is_valid()
        );
        assert!(
            !Timestamp {
                month: 11,
                day: 31,
                ..ok
            }
            .is_valid()
        );
        assert!(
            !Timestamp {
                year: 2100,
                month: 2,
                day: 29,
                ..ok
            }
            .is_valid()
        ); // leap year
    }
}

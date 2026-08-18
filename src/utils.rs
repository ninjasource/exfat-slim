use core::str::EncodeUtf16;

use aligned::Aligned;

use super::{
    BlockDevice, bisync, boot_sector::VolumeFlags, error::ExFatError, file_system::ExFatResult,
    upcase_table::UpcaseTable,
};

pub(crate) fn read_u16_le<const INDEX: usize, const N: usize>(value: &[u8; N]) -> u16 {
    let mut tmp = [0u8; size_of::<u16>()];
    tmp.copy_from_slice(&value[INDEX..INDEX + size_of::<u16>()]);
    u16::from_le_bytes(tmp)
}

pub(crate) fn read_u32_le<const INDEX: usize, const N: usize>(value: &[u8; N]) -> u32 {
    let mut tmp = [0u8; size_of::<u32>()];
    tmp.copy_from_slice(&value[INDEX..INDEX + size_of::<u32>()]);
    u32::from_le_bytes(tmp)
}

pub(crate) fn read_u64_le<const INDEX: usize, const N: usize>(value: &[u8; N]) -> u64 {
    let mut tmp = [0u8; size_of::<u64>()];
    tmp.copy_from_slice(&value[INDEX..INDEX + size_of::<u64>()]);
    u64::from_le_bytes(tmp)
}

/// calculate the number of 32 byte chunks required for the directory entry set
pub(crate) fn calc_dir_entry_set_len(name_char_count: usize) -> usize {
    // file_dir + stream_extension + file_name entries in blocks of 15 characters
    2 + (name_char_count as u32).div_ceil(15) as usize
}

pub(crate) fn encode_utf16_upcase_and_hash(
    s: &str,
    upcase_table: &UpcaseTable,
) -> Result<(u16, usize), &'static str> {
    if s == "." || s == ".." {
        return Err(". and .. are reserved names");
    }

    let mut hash = 0u16;
    let mut count: usize = 0;
    for c in s.encode_utf16() {
        count += 1;
        if is_illegal_name_char(c) {
            return Err("name contains an illegal character");
        }
        let c = upcase_table.upcase(c);
        let byte0 = (c & 0xFF) as u8;
        let byte1 = (c >> 8) as u8;
        hash = if hash & 1 > 0 { 0x8000 } else { 0 } + hash.wrapping_shr(1) + byte0 as u16;
        hash = if hash & 1 > 0 { 0x8000 } else { 0 } + hash.wrapping_shr(1) + byte1 as u16;
    }

    if count == 0 {
        return Err("name is empty");
    }

    Ok((hash, count))
}

pub(crate) fn split_path(path: &str) -> (&str, &str) {
    path.rfind(['/', '\\'])
        .map_or(("", path), |index| (&path[..index], &path[index + 1..]))
}

#[bisync]
pub async fn set_volume_dirty<D, const SIZE: usize>(
    io: &mut D,
    is_dirty: bool,
) -> ExFatResult<(), D, SIZE>
where
    D: BlockDevice<SIZE>,
{
    let sector_id = 0; // boot sector
    let mut block = [Aligned([0u8; SIZE])];
    io.read(sector_id, &mut block)
        .await
        .map_err(ExFatError::Io)?;
    let mut volume_flags = VolumeFlags::from_bits_truncate(read_u16_le::<106, _>(&block[0]));
    volume_flags.set(VolumeFlags::VolumeDirty, is_dirty);
    block[0][106..108].copy_from_slice(&volume_flags.bits().to_le_bytes());
    io.write(sector_id, &block).await.map_err(ExFatError::Io)?;
    Ok(())
}

pub(crate) struct Utf16Chunks<'a> {
    iter: EncodeUtf16<'a>,
}

impl<'a> Utf16Chunks<'a> {
    pub fn new(s: &'a str) -> Self {
        let iter = s.encode_utf16();
        Self { iter }
    }

    pub fn next_chunk<const N: usize>(&mut self, chunk: &mut [u16; N]) -> Option<usize> {
        assert!(N > 0);
        let mut index = 0;

        while index < N {
            match self.iter.next() {
                Some(item) => {
                    chunk[index] = item;
                    index += 1;
                }
                None => break,
            }
        }

        if index == 0 {
            None
        } else {
            chunk[index..].fill(0);
            Some(index)
        }
    }
}

pub(crate) fn is_illegal_name_char(c: u16) -> bool {
    // all 32 ascii control characters and
    // "*/:<>?\|
    c <= 0x001F
        || matches!(
            c,
            0x0022 | 0x002A | 0x002F | 0x003A | 0x003C | 0x003E | 0x003F | 0x005C | 0x007C
        )
}

#[allow(unused)]
#[cfg(test)]
mod tests {
    use super::super::only_sync;
    use super::*;
    use aligned::Aligned;
    use alloc::{vec, vec::Vec};

    #[only_sync]
    #[test]
    fn directory_entry_name_longer_than_15_utf16_units() {
        // 20 utf16 units
        let mut chunks = Utf16Chunks::new("abcdefghij0123456789ABCDE");
        let mut buf = [0u16; 15];
        assert_eq!(chunks.next_chunk(&mut buf), Some(15));
        assert_eq!(buf[14], '4' as u16);
        assert_eq!(chunks.next_chunk(&mut buf), Some(10));
        assert_eq!(buf[0], '5' as u16);
        assert_eq!(&buf[10..], [0u16; 5]); // zero padding
        assert_eq!(chunks.next_chunk(&mut buf), None)
    }

    #[only_sync]
    #[test]
    fn directory_entry_name_less_than_one_full_chunk() {
        // 15 utf16 units
        let mut chunks = Utf16Chunks::new("0123456789");
        let mut buf = [0u16; 15];
        assert_eq!(chunks.next_chunk(&mut buf), Some(10));
        assert_eq!(chunks.next_chunk(&mut buf), None)
    }

    #[only_sync]
    #[test]
    fn directory_entry_name_one_full_chunk() {
        // 10 utf16 units
        let mut chunks = Utf16Chunks::new("012345678901234");
        let mut buf = [0u16; 15];
        assert_eq!(chunks.next_chunk(&mut buf), Some(15));
        assert_eq!(chunks.next_chunk(&mut buf), None)
    }

    #[only_sync]
    #[test]
    fn calc_dir_entry_set_len_boundaries() {
        assert_eq!(calc_dir_entry_set_len(1), 3);
        assert_eq!(calc_dir_entry_set_len(15), 3);
        assert_eq!(calc_dir_entry_set_len(16), 4);
        assert_eq!(calc_dir_entry_set_len(30), 4);
        assert_eq!(calc_dir_entry_set_len(31), 5);
    }

    #[only_sync]
    #[test]
    fn encode_utf16_upcase_and_hash_tests() {
        let upcase_table = UpcaseTable::default();
        assert_eq!(
            (0x3046, 9),
            encode_utf16_upcase_and_hash("Hello.TXT", &upcase_table).unwrap()
        );
        assert_eq!(
            (0xEF63, 29),
            encode_utf16_upcase_and_hash("This IS A LONG folder NAME !!", &upcase_table).unwrap()
        );
        assert_eq!(
            (0x6FAD, 27),
            encode_utf16_upcase_and_hash("File name with no extension", &upcase_table).unwrap()
        );
        assert_eq!(
            (0x72B1, 82),
            encode_utf16_upcase_and_hash(
                "This is a very long file name but ok 123 - to use as an exfat name 🦀 so there.Txt",
                &upcase_table
            ).unwrap()
        );
        assert_eq!(
            (0xE28E, 254),
            encode_utf16_upcase_and_hash(
                "Here is an example of an extremely long file name but it should still be compatible with the exfat file SYSTEM even though nobody would EVERY use a filename like this right. Am I right. Maybe Im wrong - maybe you get those that dont even use an extension",
                &upcase_table
            ).unwrap()
        );
    }

    #[only_sync]
    #[test]
    fn invalid_file_names() {
        let upcase_table = UpcaseTable::default();
        assert!(encode_utf16_upcase_and_hash("", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash(".", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("..", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("\0", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("hello*", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("hello\"", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("hello/", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("hello:", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("hello<", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("hello>", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("hello?", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("hello\\", &upcase_table).is_err());
        assert!(encode_utf16_upcase_and_hash("hello|", &upcase_table).is_err());
    }

    #[only_sync]
    #[test]
    fn split_path_edge_cases() {
        assert_eq!(split_path("a/b/c.txt"), ("a/b", "c.txt"));
        assert_eq!(split_path("c.txt"), ("", "c.txt"));
        assert_eq!(split_path("/c.txt"), ("", "c.txt"));
        assert_eq!(split_path("a\\b\\c.txt"), ("a\\b", "c.txt"));
        assert_eq!(split_path(" c.txt"), ("", " c.txt"));
        assert_eq!(split_path("c.txt "), ("", "c.txt "));
        assert_eq!(split_path("  c.txt  "), ("", "  c.txt  "));
        assert_eq!(split_path(" a / b / c.txt "), (" a / b ", " c.txt "));
        assert_eq!(split_path("a/b/"), ("a/b", ""));
    }
}

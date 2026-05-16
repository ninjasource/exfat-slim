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

pub(crate) fn encode_utf16_upcase_and_hash(s: &str, upcase_table: &UpcaseTable) -> (u16, usize) {
    let mut hash = 0u16;
    let mut count: usize = 0;
    for c in s.encode_utf16() {
        count += 1;
        let c = upcase_table.upcase(c);
        let byte0 = (c & 0xFF) as u8;
        let byte1 = (c >> 8) as u8;
        hash = if hash & 1 > 0 { 0x8000 } else { 0 } + hash.wrapping_shr(1) + byte0 as u16;
        hash = if hash & 1 > 0 { 0x8000 } else { 0 } + hash.wrapping_shr(1) + byte1 as u16;
    }

    (hash, count)
}

pub(crate) fn split_path(path: &str) -> (&str, &str) {
    path.rfind(['/', '\\']).map_or(("", path.trim()), |index| {
        (path[..index].trim(), path[index + 1..].trim())
    })
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
        for item in self.iter.by_ref() {
            if index < chunk.len() {
                chunk[index] = item;
                index += 1;
            } else {
                return None;
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

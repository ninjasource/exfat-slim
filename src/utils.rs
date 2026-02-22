use alloc::{string::String, vec, vec::Vec};

use super::{
    bisync,
    boot_sector::VolumeFlags,
    error::ExFatError,
    io::{BLOCK_SIZE, BlockDevice},
    upcase_table::UpcaseTable,
};

pub(crate) fn _chunk_at<const N: usize, T>(slice: &[T], index: usize) -> Option<&[T; N]> {
    let start = index.checked_mul(N)?;
    let end = start.checked_add(N)?;
    slice.get(start..end)?.try_into().ok()
}

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
pub(crate) fn calc_dir_entry_set_len(name: &[u16]) -> usize {
    // file_dir + stream_extension + file_name entries in blocks of 15 characters
    2 + (name.len() as u32).div_ceil(15) as usize
}

pub(crate) fn encode_utf16_and_hash(s: &str, upcase_table: &UpcaseTable) -> (Vec<u16>, u16) {
    let mut file_name: Vec<u16> = s.encode_utf16().collect();
    for c in file_name.iter_mut() {
        *c = upcase_table.upcase(*c)
    }
    let file_name_hash = calc_hash_u16(file_name.as_slice());

    // this copy is not upcased
    let file_name: Vec<u16> = s.encode_utf16().collect();

    (file_name, file_name_hash)
}

pub(crate) fn encode_utf16_upcase_and_hash(s: &str, upcase_table: &UpcaseTable) -> (Vec<u16>, u16) {
    let mut file_name: Vec<u16> = s.encode_utf16().collect();
    for c in file_name.iter_mut() {
        *c = upcase_table.upcase(*c)
    }
    let file_name_hash = calc_hash_u16(file_name.as_slice());

    (file_name, file_name_hash)
}

pub(crate) fn calc_hash_u16(utf16_file_name: &[u16]) -> u16 {
    let mut hash = 0u16;

    for byte in utf16_file_name
        .iter()
        .flat_map(|x| vec![(x & 0xFF) as u8, (x >> 8) as u8])
    {
        hash = if hash & 1 > 0 { 0x8000 } else { 0 } + hash.wrapping_shr(1) + byte as u16;
    }

    hash
}

pub(crate) fn _decode_utf16(buf: Vec<u16>) -> Result<String, ExFatError> {
    let decoded = core::char::decode_utf16(buf)
        .map(|r| {
            // TODO reject illegal character like quotes (see spec)
            r.map_err(|_| ExFatError::InvalidUtf16String {
                reason: "invalid u16 char detected",
            })
        })
        .collect::<Result<String, ExFatError>>()?;
    Ok(decoded)
}

#[bisync]
pub async fn set_volume_dirty(io: &mut impl BlockDevice, is_dirty: bool) -> Result<(), ExFatError> {
    let sector_id = 0; // boot sector
    let mut block = [0u8; BLOCK_SIZE];
    io.read_sector(sector_id, &mut block).await?;
    let mut volume_flags = VolumeFlags::from_bits_truncate(read_u16_le::<106, _>(&block));
    volume_flags.set(VolumeFlags::VolumeDirty, is_dirty);
    block[106..108].copy_from_slice(&volume_flags.bits().to_le_bytes());
    io.write_sector(sector_id, &block).await?;
    Ok(())
}

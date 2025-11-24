use alloc::{string::String, vec, vec::Vec};

use crate::error::ExFatError;

pub fn read_u16_le<const INDEX: usize, const N: usize>(value: &[u8; N]) -> u16 {
    let mut tmp = [0u8; size_of::<u16>()];
    tmp.copy_from_slice(&value[INDEX..INDEX + size_of::<u16>()]);
    u16::from_le_bytes(tmp)
}

pub fn read_u32_le<const INDEX: usize, const N: usize>(value: &[u8; N]) -> u32 {
    let mut tmp = [0u8; size_of::<u32>()];
    tmp.copy_from_slice(&value[INDEX..INDEX + size_of::<u32>()]);
    u32::from_le_bytes(tmp)
}

pub fn read_u64_le<const INDEX: usize, const N: usize>(value: &[u8; N]) -> u64 {
    let mut tmp = [0u8; size_of::<u64>()];
    tmp.copy_from_slice(&value[INDEX..INDEX + size_of::<u64>()]);
    u64::from_le_bytes(tmp)
}

pub fn calc_hash_u16(utf16_file_name: &[u16]) -> u16 {
    let mut hash = 0u16;

    for byte in utf16_file_name
        .iter()
        .flat_map(|x| vec![(x & 0xFF) as u8, (x >> 8) as u8])
    {
        hash = if hash & 1 > 0 { 0x8000 } else { 0 } + hash.wrapping_shr(1) + byte as u16;
    }

    hash
}

pub fn decode_utf16(buf: Vec<u16>) -> Result<String, ExFatError> {
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

use bitflags::bitflags;
use thiserror::Error;

use crate::{
    error::ExFatError,
    fat::next_cluster_in_fat_chain,
    file_system::FileSystemDetails,
    io::{BLOCK_SIZE, BlockDevice},
    utils::{read_u16_le, read_u32_le, read_u64_le},
};

pub const RAW_ENTRY_LEN: usize = 32;
pub const DIR_ENTRIES_PER_BLOCK: usize = BLOCK_SIZE / RAW_ENTRY_LEN;

#[derive(Error, Debug)]
pub enum Error {
    #[error("invalid uft16 string encountered ({reason})")]
    InvalidUtf16String { reason: &'static str },
}

/// Entry Type (identifies what kind of 32 byte entry this is)
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EntryType {
    UnusedOrEndOfDirectory,
    AllocationBitmap,
    UpcaseTable,
    VolumeLabel,
    FileAndDirectory,
    VolumeGuid,
    TexFATPadding,
    StreamExtension,
    Filename,
    Reserved(u8),
}

/// Allocation bitmap
#[derive(Debug)]
pub struct AllocationBitmapDirEntry {
    pub bitmap_flags: BitmapFlags,
    pub first_cluster: u32,
    pub data_length: u64,
}

/// Up-case table
#[derive(Debug)]
pub struct UpcaseTableDirEntry {
    pub table_checksum: u32,
    pub first_cluster: u32,
    pub data_length: u64,
}

/// Volume label
#[derive(Debug)]
pub struct VolumeLabelDirEntry(pub heapless::String<22>); // 11 characters

/// File and directory (file attribute and timestamp) also known as DirectoryEntry
#[derive(Debug)]
pub struct FileDirEntry {
    pub secondary_count: u8, // the number of entries following this one
    pub set_checksum: u16,
    pub file_attributes: FileAttributes,
    pub create_timestamp: u32,
    pub last_modified_timestamp: u32,
    pub last_accessed_timestamp: u32,
    pub create_10ms_increment: u8,
    pub last_modified_10ms_increment: u8,
    pub create_utc_offset: u8,
    pub last_modified_utc_offset: u8,
    pub last_accessed_utc_offset: u8,
}

/// Stream extension (file allocation information)
#[derive(Debug)]
pub struct StreamExtensionDirEntry {
    pub general_secondary_flags: GeneralSecondaryFlags,
    pub name_length: u8,
    pub name_hash: u16,
    pub valid_data_length: u64,
    pub first_cluster: u32,
    pub data_length: u64,
}

/// File name (name of the file - part)
#[derive(Debug)]
pub struct FileNameDirEntry {
    pub general_secondary_flags: GeneralSecondaryFlags,
    pub file_name: [u16; 15], // utf16 formatted
}

/*
impl Into<u8> for EntryType {
    fn into(self) -> u8 {
        match self {
            Self::UnusedOrEndOfDirectory => 0x00,
            Self::AllocationBitmap => 0x81,
            Self::UpcaseTable => 0x82,
            Self::VolumeLabel => 0x83,
            Self::FileAndDirectory => 0x85,
            Self::VolumeGuid => 0xA0,
            Self::TexFATPadding => 0xA1,
            Self::StreamExtension => 0xC0,
            Self::Filename => 0xC1,
            Self::Reserved(x) => x,
        }
    }
}*/

impl From<u8> for EntryType {
    fn from(value: u8) -> Self {
        match value {
            0x00 => Self::UnusedOrEndOfDirectory,
            0x81 => Self::AllocationBitmap,
            0x82 => Self::UpcaseTable,
            0x83 => Self::VolumeLabel,
            0x85 => Self::FileAndDirectory,
            0xA0 => Self::VolumeGuid,
            0xA1 => Self::TexFATPadding,
            0xC0 => Self::StreamExtension,
            0xC1 => Self::Filename,
            x => Self::Reserved(x),
        }
    }
}

impl From<&[u8; RAW_ENTRY_LEN]> for AllocationBitmapDirEntry {
    fn from(value: &[u8; RAW_ENTRY_LEN]) -> Self {
        let bitmap_flags = BitmapFlags::from_bits_truncate(value[1]);
        let first_cluster = read_u32_le::<20, _>(value);
        let data_length = read_u64_le::<24, _>(value);
        Self {
            bitmap_flags,
            first_cluster,
            data_length,
        }
    }
}

impl From<&[u8; RAW_ENTRY_LEN]> for UpcaseTableDirEntry {
    fn from(value: &[u8; RAW_ENTRY_LEN]) -> Self {
        let table_checksum = read_u32_le::<4, _>(value);
        let first_cluster = read_u32_le::<20, _>(value);
        let data_length = read_u64_le::<24, _>(value);

        Self {
            table_checksum,
            first_cluster,
            data_length,
        }
    }
}

impl TryFrom<&[u8; RAW_ENTRY_LEN]> for VolumeLabelDirEntry {
    type Error = Error;

    fn try_from(value: &[u8; RAW_ENTRY_LEN]) -> Result<Self, Self::Error> {
        let character_count = value[1] as usize;
        if character_count > 11 {
            return Err(Error::InvalidUtf16String {
                reason: "character count exceeds 11",
            });
        }
        let character_num_bytes = character_count * 2; // utf-16 encoded

        let volume_label = decode_utf16_le(&value[2..2 + character_num_bytes])?;
        Ok(Self(volume_label))
    }
}

impl From<&[u8; RAW_ENTRY_LEN]> for FileDirEntry {
    fn from(value: &[u8; RAW_ENTRY_LEN]) -> Self {
        let secondary_count = value[1];
        let set_checksum = read_u16_le::<2, _>(value);
        let file_attributes = FileAttributes::from_bits_truncate(read_u16_le::<4, _>(value));
        let create_timestamp = read_u32_le::<8, _>(value);
        let last_modified_timestamp = read_u32_le::<12, _>(value);
        let last_accessed_timestamp = read_u32_le::<16, _>(value);
        let create_10ms_increment = value[20];
        let last_modified_10ms_increment = value[21];
        let create_utc_offset = value[22];
        let last_modified_utc_offset = value[23];
        let last_accessed_utc_offset = value[24];

        Self {
            secondary_count,
            set_checksum,
            file_attributes,
            create_timestamp,
            last_modified_timestamp,
            last_accessed_timestamp,
            create_10ms_increment,
            last_modified_10ms_increment,
            create_utc_offset,
            last_modified_utc_offset,
            last_accessed_utc_offset,
        }
    }
}

impl From<&[u8; RAW_ENTRY_LEN]> for StreamExtensionDirEntry {
    fn from(value: &[u8; RAW_ENTRY_LEN]) -> Self {
        let general_secondary_flags = GeneralSecondaryFlags::from_bits_truncate(value[1]);
        let name_length = value[3];
        let name_hash = read_u16_le::<4, _>(value);
        let valid_data_length = read_u64_le::<8, _>(value);
        let first_cluster = read_u32_le::<20, _>(value);
        let data_length = read_u64_le::<24, _>(value);

        Self {
            general_secondary_flags,
            name_length,
            name_hash,
            valid_data_length,
            first_cluster,
            data_length,
        }
    }
}

impl From<&[u8; RAW_ENTRY_LEN]> for FileNameDirEntry {
    fn from(value: &[u8; RAW_ENTRY_LEN]) -> Self {
        let general_secondary_flags = GeneralSecondaryFlags::from_bits_truncate(value[1]);
        let mut file_name: [u16; 15] = [0; 15];
        let (chunks, _remainder) = value[2..RAW_ENTRY_LEN].as_chunks::<2>();
        let u16_iter = chunks.iter().map(|x| u16::from_le_bytes(*x));
        for (from, to) in u16_iter.zip(file_name.iter_mut()) {
            *to = from;
        }

        Self {
            general_secondary_flags,
            file_name,
        }
    }
}

#[derive(Debug)]
pub enum BitmapIdentifier {
    FirstAllocation,
    SecondAllocation,
}

bitflags! {
    /// Represents a set of bitmap flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct BitmapFlags: u8 {
        /// The value `FirstOrSecondBitmap`, at bit position `0`.
        /// 0 = 1st bitmap
        /// 1 = 2nd bitmap
        const FirstOrSecondBitmap = 0b0000_0001;
    }


    /// Represents a set of volume flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct GeneralSecondaryFlags: u8 {
        /// The value `AllocationPossible`, at bit position `0`.
        /// 0 = Cluster allocation is not possible and FirstCluster and DataLength field are undefined,
        /// 1 = Cluster allocation is possible and FirstCluster and DataLength field are valid as defined.
        const AllocationPossible = 0b0000_0001;

        /// The value `NoFatChain`, at bit position `1`.
        /// 0 = Cluster chain on the FAT is valid, 1 = Cluster chain is contiguous and not recorded on the FAT.
        const NoFatChain = 0b0000_0010;
    }

    /// Represents a set of file attributes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct FileAttributes: u16 {
        /// The value `ReadOnly`, at bit position `0`.
        const ReadOnly = 0b0000_0001;

        /// The value `ReadOnly`, at bit position `1`.
        const Hidden = 0b0000_0010;

        /// The value `ReadOnly`, at bit position `2`.
        const System = 0b0000_0100;

        /// The value `ReadOnly`, at bit position `4`.
        const Directory = 0b0001_0000;

        /// The value `ReadOnly`, at bit position `5`.
        const Archive = 0b0010_0000;
    }
}

fn decode_utf16_le<const N: usize>(bytes: &[u8]) -> Result<heapless::String<N>, Error> {
    let (chunks, _remainder) = bytes.as_chunks::<2>();
    let u16_iter = chunks.iter().map(|x| u16::from_le_bytes(*x));

    let decoded = core::char::decode_utf16(u16_iter)
        .map(|r| {
            // TODO reject illegal character like quotes (see spec)
            r.map_err(|_| Error::InvalidUtf16String {
                reason: "invalid u16 char detected",
            })
        })
        .collect::<Result<heapless::String<N>, _>>()?;
    Ok(decoded)
}

pub struct DirectoryEntryChain {
    cluster_id: u32,
    fs: FileSystemDetails,
    cluster_offset: usize,
    dir_entry_offset: usize,
    buf: [u8; BLOCK_SIZE],
    fetch_required: bool,
}

impl DirectoryEntryChain {
    pub fn new(cluster_id: u32, fs: &FileSystemDetails) -> Self {
        Self {
            buf: [0; BLOCK_SIZE],
            fs: fs.clone(),
            cluster_id,
            cluster_offset: 0,
            dir_entry_offset: 0,
            fetch_required: true,
        }
    }

    fn get_current_sector_id(&self) -> Result<u32, ExFatError> {
        let mut sector_id = self.fs.get_heap_sector_id(self.cluster_id)?;
        sector_id += self.cluster_offset as u32;

        Ok(sector_id)
    }

    pub async fn next<'a>(
        &'a mut self,
        io: &mut impl BlockDevice,
    ) -> Result<Option<&'a [u8; RAW_ENTRY_LEN]>, ExFatError> {
        if self.dir_entry_offset >= DIR_ENTRIES_PER_BLOCK {
            self.cluster_offset += 1;
            self.dir_entry_offset = 0;
            self.fetch_required = true;
        }

        if self.cluster_offset > self.fs.sectors_per_cluster as usize {
            // we have reached the end of the cluster
            let cluster_id =
                next_cluster_in_fat_chain(self.fs.fat_offset, self.cluster_id, io).await?;
            match cluster_id {
                Some(cluster_id) => {
                    self.cluster_id = cluster_id;
                    self.cluster_offset = 0;
                    self.dir_entry_offset = 0;
                    self.fetch_required = true;
                }
                None => return Ok(None),
            }
        }

        if self.fetch_required {
            let sector_id = self.get_current_sector_id()?;
            let buf = io.read_sector(sector_id).await?;
            self.buf.copy_from_slice(buf);
            self.fetch_required = false;
        }

        let (entries, _remainder) = self.buf.as_chunks::<RAW_ENTRY_LEN>();
        let entry = &entries[self.dir_entry_offset];
        self.dir_entry_offset += 1;
        Ok(Some(entry))
    }
}

pub async fn next_file_dir_entry(
    io: &mut impl BlockDevice,
    entries: &mut DirectoryEntryChain,
) -> Result<Option<FileDirEntry>, ExFatError> {
    while let Some(entry) = entries.next(io).await? {
        let entry_type_val = entry[0];
        match EntryType::from(entry_type_val) {
            EntryType::UnusedOrEndOfDirectory => {
                if is_end_of_directory(entry) {
                    return Ok(None);
                }
            }
            EntryType::FileAndDirectory => {
                let file_entry: FileDirEntry = entry.into();
                return Ok(Some(file_entry));
            }
            _entry_type => {} // ignore and keep going
        }
    }

    Ok(None)
}

pub fn is_end_of_directory(directory_entry: &[u8; 32]) -> bool {
    // all bytes in the entry must be zero for this to be an end of directory marker
    directory_entry.iter().all(|&x| x == 0)
}

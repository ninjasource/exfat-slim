use core::char::decode_utf16;

use bitflags::bitflags;
use thiserror::Error;

use crate::timestamp::EncodedTimestamp;

use super::{
    BlockDevice, bisync,
    directory::DirectoryEntryFilter,
    error::ExFatError,
    file::{FileDetails, Touched, TouchedKind, TouchedSector},
    file_system::{ExFatResult, FileSystem, FileSystemDetails},
    utils::{read_u16_le, read_u32_le, read_u64_le},
};

pub const RAW_ENTRY_LEN: usize = 32;
//pub const DIR_ENTRIES_PER_BLOCK: usize = BLOCK_SIZE / RAW_ENTRY_LEN;

pub type RawDirEntry = [u8; RAW_ENTRY_LEN];

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Error, Debug, Clone, Copy)]
pub enum Error {
    #[error("invalid utf16 string encountered ({reason})")]
    InvalidUtf16String { reason: &'static str },
}

/// Entry Type (identifies what kind of 32 byte entry this is)
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum EntryType {
    EndOfDirectory,
    Unused(u8),
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
pub(crate) struct AllocationBitmapDirEntry {
    pub _bitmap_flags: BitmapFlags,
    pub first_cluster: u32,
    /// size, in bytes, of the allocation bitmap
    pub data_length: u64,
}

/// Up-case table
#[derive(Debug)]
pub(crate) struct UpcaseTableDirEntry {
    pub _table_checksum: u32,
    pub first_cluster: u32,
    pub _data_length: u64,
}

/// Volume label
#[derive(Debug)]
#[allow(unused)]
pub(crate) struct VolumeLabelDirEntry(pub heapless::String<22>); // 11 characters

/// File and directory (file attribute and timestamp) also known as DirectoryEntry
#[derive(Debug)]
pub(crate) struct FileDirEntry {
    /// the number of entries following this one
    pub secondary_count: u8,

    /// the checksum of all the directory entries in this set (excluding this field)
    pub set_checksum: u16,

    /// file or directory flags like Directory or Archive for example
    pub file_attributes: FileAttributes,

    /// local date and time of creation of the entry set (see spec for bit offsets - it is NOT a unix timestamp)
    pub create_timestamp: u32,

    /// local date and time that any of the clusters associated with the stream extension were last modified
    pub last_modified_timestamp: u32,

    /// local date and time that any of the clusters associated with the stream extension were last modified or read
    pub last_accessed_timestamp: u32,

    /// extra resolution for create timestamp (0-199 = 0ms-1990ms)
    pub create_10ms_increment: u8,

    /// extra resolution for modify timestamp (0-199 = 0ms-1990ms)
    pub last_modified_10ms_increment: u8,

    /// utc offset of the local time for the create timestamp
    pub create_utc_offset: u8,

    /// utc offset of the local time for the modify timestamp
    pub last_modified_utc_offset: u8,

    /// utc offset of the local time for the last accessed timestamp
    pub last_accessed_utc_offset: u8,
}

impl FileDirEntry {
    pub(crate) fn serialize(&self) -> RawDirEntry {
        let mut raw = [0u8; RAW_ENTRY_LEN];
        raw[0] = EntryType::FileAndDirectory.serialize();
        raw[1] = self.secondary_count;
        raw[2..4].copy_from_slice(&self.set_checksum.to_le_bytes());
        raw[4..6].copy_from_slice(&self.file_attributes.bits().to_le_bytes());
        raw[8..12].copy_from_slice(&self.create_timestamp.to_le_bytes());
        raw[12..16].copy_from_slice(&self.last_modified_timestamp.to_le_bytes());
        raw[16..20].copy_from_slice(&self.last_accessed_timestamp.to_le_bytes());
        raw[20] = self.create_10ms_increment;
        raw[21] = self.last_modified_10ms_increment;
        raw[22] = self.create_utc_offset;
        raw[23] = self.last_modified_utc_offset;
        raw[24] = self.last_accessed_utc_offset;
        raw
    }

    pub(crate) fn created(&self) -> EncodedTimestamp {
        EncodedTimestamp {
            packed: self.create_timestamp,
            increment_10ms: self.create_10ms_increment,
            utc_offset: self.create_utc_offset,
        }
    }

    pub(crate) fn modified(&self) -> EncodedTimestamp {
        EncodedTimestamp {
            packed: self.last_modified_timestamp,
            increment_10ms: self.last_modified_10ms_increment,
            utc_offset: self.last_modified_utc_offset,
        }
    }

    pub(crate) fn accessed(&self) -> EncodedTimestamp {
        EncodedTimestamp {
            packed: self.last_accessed_timestamp,
            increment_10ms: 0, // none for accessed
            utc_offset: self.last_accessed_utc_offset,
        }
    }
}

/// Stream extension (file allocation information)
#[derive(Debug)]
pub(crate) struct StreamExtensionDirEntry {
    pub general_secondary_flags: GeneralSecondaryFlags,

    // length, in number of characters, of the unicode string (range 1-255 is valid)
    pub name_length: u8,

    /// hash of the upcased filename
    pub name_hash: u16,

    /// how far into the data stream the user data has been written in number of bytes.
    /// if the user requests data beyond the valid data length then zeros must be supplied.
    pub valid_data_length: u64,

    /// first cluster of the data stream
    pub first_cluster: u32,

    /// size, in bytes, of the data the associated cluster allocation contains
    pub data_length: u64,
}

impl StreamExtensionDirEntry {
    pub(crate) fn serialize(&self) -> RawDirEntry {
        let mut raw = [0u8; RAW_ENTRY_LEN];
        raw[0] = EntryType::StreamExtension.serialize();
        raw[1] = self.general_secondary_flags.bits();
        raw[3] = self.name_length;
        raw[4..6].copy_from_slice(&self.name_hash.to_le_bytes());
        raw[8..16].copy_from_slice(&self.valid_data_length.to_le_bytes());
        raw[20..24].copy_from_slice(&self.first_cluster.to_le_bytes());
        raw[24..32].copy_from_slice(&self.data_length.to_le_bytes());
        raw
    }
}

/// File name (name of the file - part)
#[derive(Debug)]
pub(crate) struct FileNameDirEntry {
    pub general_secondary_flags: GeneralSecondaryFlags,
    pub file_name: [u16; 15], // utf16 formatted
}

impl FileNameDirEntry {
    pub(crate) fn serialize(&self) -> RawDirEntry {
        let mut raw = [0u8; RAW_ENTRY_LEN];
        raw[0] = EntryType::Filename.serialize();
        raw[1] = self.general_secondary_flags.bits();

        let (chunks, _remainder) = raw[2..32].as_chunks_mut::<2>();
        for (to, from) in chunks.iter_mut().zip(&self.file_name) {
            to.copy_from_slice(&from.to_le_bytes());
        }
        raw
    }
}

impl From<u8> for EntryType {
    fn from(value: u8) -> Self {
        match value {
            0x00 => Self::EndOfDirectory,
            // InUse bit clear and holds the original byte
            x if x & 0x80 == 0 => Self::Unused(x),
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

impl EntryType {
    pub(crate) fn serialize(&self) -> u8 {
        match self {
            Self::EndOfDirectory => 0x00,
            Self::Unused(x) => *x,
            Self::AllocationBitmap => 0x81,
            Self::UpcaseTable => 0x82,
            Self::VolumeLabel => 0x83,
            Self::FileAndDirectory => 0x85,
            Self::VolumeGuid => 0xA0,
            Self::TexFATPadding => 0xA1,
            Self::StreamExtension => 0xC0,
            Self::Filename => 0xC1,
            Self::Reserved(x) => *x,
        }
    }
}

impl From<&[u8; RAW_ENTRY_LEN]> for AllocationBitmapDirEntry {
    fn from(value: &[u8; RAW_ENTRY_LEN]) -> Self {
        let _bitmap_flags = BitmapFlags::from_bits_truncate(value[1]);
        let first_cluster = read_u32_le::<20, _>(value);
        let data_length = read_u64_le::<24, _>(value);
        Self {
            _bitmap_flags,
            first_cluster,
            data_length,
        }
    }
}

impl From<&[u8; RAW_ENTRY_LEN]> for UpcaseTableDirEntry {
    fn from(value: &[u8; RAW_ENTRY_LEN]) -> Self {
        let _table_checksum = read_u32_le::<4, _>(value);
        let first_cluster = read_u32_le::<20, _>(value);
        let _data_length = read_u64_le::<24, _>(value);

        Self {
            _table_checksum,
            first_cluster,
            _data_length,
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

#[cfg(feature = "defmt")]
impl defmt::Format for FileAttributes {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(f, "FileAttributes({=u16:#010b})", self.bits());
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for GeneralSecondaryFlags {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(f, "GeneralSecondaryFlags({=u8:#010b})", self.bits());
    }
}

bitflags! {
    /// Represents a set of bitmap flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub(crate) struct BitmapFlags: u8 {
        /// The value `FirstOrSecondBitmap`, at bit position `0`.
        /// 0 = 1st bitmap
        /// 1 = 2nd bitmap
        const FirstOrSecondBitmap = 0b0000_0001;
    }


    /// Represents a set of volume flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub(crate) struct GeneralSecondaryFlags: u8 {
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
    pub(crate) struct FileAttributes: u16 {
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

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Location {
    /// the absolute sector_id.
    /// all sectors in a cluster are contiguous
    pub sector_id: u32,

    /// number of 32 byte directory entries to skip
    pub dir_entry_offset: usize,
}

impl Location {
    pub(crate) fn new(sector_id: u32, dir_entry_offset: usize) -> Self {
        Self {
            sector_id,
            dir_entry_offset,
        }
    }
}

fn decode_utf16_le<const N: usize>(bytes: &[u8]) -> Result<heapless::String<N>, Error> {
    let (chunks, _remainder) = bytes.as_chunks::<2>();
    let u16_iter = chunks.iter().map(|x| u16::from_le_bytes(*x));

    let decoded = core::char::decode_utf16(u16_iter)
        .map(|r| {
            r.map_err(|_| Error::InvalidUtf16String {
                reason: "invalid u16 char detected",
            })
        })
        .collect::<Result<heapless::String<N>, _>>()?;
    Ok(decoded)
}

pub(crate) struct DirectoryEntryChain<const SIZE: usize> {
    cluster_id: u32,
    fs: FileSystemDetails,
    // offset, in number of sectors, from start of cluster
    cluster_offset: usize,
    // offset, in number of RAW_ENTRY_LEN chunks, from start of sector
    dir_entry_offset: usize,
    buf: [u8; SIZE], // TODO: figure out if this is still needed after slot cache was introduced
    fetch_required: bool,
    cursor: usize,
    num_entries: Option<usize>,
}

impl<const SIZE: usize> DirectoryEntryChain<SIZE> {
    pub(crate) fn new_from_file_details(details: &FileDetails, fs: &FileSystemDetails) -> Self {
        // This is so gross, make it better
        let sector_id_from_start = details.location.sector_id - fs.cluster_heap_offset;
        let cluster_id = 2 + sector_id_from_start / fs.sectors_per_cluster;
        let cluster_offset = (sector_id_from_start % fs.sectors_per_cluster) as usize;
        let dir_entry_offset = details.location.dir_entry_offset;
        let num_entries = Some(details.secondary_count as usize + 1);

        Self {
            buf: [0; SIZE],
            fs: fs.clone(),
            cluster_id,
            cluster_offset,
            dir_entry_offset,
            fetch_required: true,
            cursor: 0,
            num_entries,
        }
    }

    pub(crate) fn new(cluster_id: u32, fs: &FileSystemDetails) -> Self {
        Self {
            buf: [0; SIZE],
            fs: fs.clone(),
            cluster_id,
            cluster_offset: 0,
            dir_entry_offset: 0,
            fetch_required: true,
            cursor: 0,
            num_entries: None,
        }
    }

    #[bisync]
    pub(crate) async fn next_file_dir_entry<D, const N: usize>(
        &mut self,
        fs: &mut FileSystem<D, SIZE, N>,
    ) -> ExFatResult<Option<(FileDirEntry, Location)>, D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        while let Some((entry, location)) = self.next(fs).await? {
            let entry_type_val = entry[0];
            match EntryType::from(entry_type_val) {
                EntryType::EndOfDirectory if is_end_of_directory(entry) => {
                    return Ok(None);
                }
                EntryType::FileAndDirectory => {
                    let file_entry: FileDirEntry = entry.into();
                    return Ok(Some((file_entry, location)));
                }
                _entry_type => {} // ignore and keep going
            }
        }

        Ok(None)
    }

    // returns the number of bytes needed to store all utf8 encoded characters for the
    // file or directory name. Zero if name_buff is None. In that case file name decoding is skipped.
    #[bisync]
    pub(crate) async fn next_file_entry<D, const N: usize>(
        &mut self,
        fs: &mut FileSystem<D, SIZE, N>,
        filter: &impl DirectoryEntryFilter,
        mut name_buf: Option<&mut [u8]>,
    ) -> ExFatResult<Option<(FileDetails, usize)>, D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        'outer: loop {
            if let Some((file_dir_entry, location)) = self.next_file_dir_entry(fs).await? {
                if let Some((stream_entry, _location)) = self.next(fs).await? {
                    let Some(stream_entry) = try_into::<StreamExtensionDirEntry>(
                        stream_entry,
                        EntryType::StreamExtension,
                    ) else {
                        return Ok(None);
                    };

                    if !filter.hash(stream_entry.name_hash, file_dir_entry.file_attributes) {
                        continue 'outer;
                    }

                    // read the entire file_name
                    let name_length = stream_entry.name_length as usize;
                    if !filter.file_name_length(name_length) {
                        continue 'outer;
                    }

                    let mut cursor: usize = 0;
                    let mut name_units = [0u16; 255];
                    'inner: loop {
                        if let Some((file_name_entry, _location)) = self.next(fs).await? {
                            let Some(file_name_entry) =
                                try_into::<FileNameDirEntry>(file_name_entry, EntryType::Filename)
                            else {
                                return Ok(None);
                            };

                            let len = (name_length - cursor).min(file_name_entry.file_name.len());
                            if !filter.file_name(
                                &file_name_entry.file_name[..len],
                                cursor,
                                &fs.upcase_table,
                            ) {
                                continue 'outer;
                            } else {
                                if name_buf.is_some() {
                                    name_units[cursor..cursor + len]
                                        .copy_from_slice(&file_name_entry.file_name[..len]);
                                }

                                cursor += len;
                                if cursor == name_length {
                                    break 'inner;
                                }
                            }
                        } else {
                            return Ok(None);
                        }
                    }

                    let file_details = FileDetails {
                        attributes: file_dir_entry.file_attributes,
                        data_length: stream_entry.data_length,
                        valid_data_length: stream_entry.valid_data_length,
                        first_cluster: stream_entry.first_cluster,
                        location,
                        flags: stream_entry.general_secondary_flags,
                        secondary_count: file_dir_entry.secondary_count,
                        accessed: file_dir_entry.accessed(),
                        created: file_dir_entry.created(),
                        modified: file_dir_entry.modified(),
                    };

                    let utf8_name_length = if let Some(name_buffer) = name_buf.as_deref_mut() {
                        decode_utf16_to_utf8::<D, SIZE, N>(&name_units[..name_length], name_buffer)?
                    } else {
                        0
                    };

                    return Ok(Some((file_details, utf8_name_length)));
                } else {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }
    }

    fn get_current_sector_id<D>(&self) -> ExFatResult<u32, D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        let mut sector_id = self.fs.get_heap_sector_id::<D, SIZE>(self.cluster_id)?;
        sector_id += self.cluster_offset as u32;
        Ok(sector_id)
    }

    const fn dir_entries_per_block() -> usize {
        SIZE / RAW_ENTRY_LEN
    }

    #[bisync]
    pub(crate) async fn next<D, const N: usize>(
        &mut self,
        fs: &mut FileSystem<D, SIZE, N>,
    ) -> ExFatResult<Option<(&RawDirEntry, Location)>, D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        // return early if we have reached the num entries in our file
        if let Some(num_entries) = self.num_entries.as_ref()
            && self.cursor == *num_entries
        {
            return Ok(None);
        }

        if self.dir_entry_offset >= Self::dir_entries_per_block() {
            self.cluster_offset += 1;
            self.dir_entry_offset = 0;
            self.fetch_required = true;
        }

        if self.cluster_offset >= self.fs.sectors_per_cluster as usize {
            // we have reached the end of the cluster
            let cluster_id = fs
                .fat
                .next_cluster_in_fat_chain(self.cluster_id, &mut fs.dev)
                .await?;
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
            let sector_id = self.get_current_sector_id::<D>()?;
            let slot = fs.data_blocks.read(sector_id, &mut fs.dev).await?;
            self.buf.copy_from_slice(slot.as_slice());
            self.fetch_required = false;
        }

        let (entries, _remainder) = self.buf.as_chunks::<RAW_ENTRY_LEN>();
        let entry = &entries[self.dir_entry_offset];
        let location = Location::new(self.get_current_sector_id::<D>()?, self.dir_entry_offset);
        self.dir_entry_offset += 1;
        self.cursor += 1;
        Ok(Some((entry, location)))
    }
}

// note that utf16 characters can span more than a single u16 (e.g. emojis)
// this is why we have to collect ALL codepoints for the filename before decoding
// otherwise we would have to keep track of previously encountered codepoints when iterating
// though a file name that is chunked between multiple dir entries
pub(crate) fn decode_utf16_to_utf8<D, const SIZE: usize, const N: usize>(
    utf16: &[u16],
    utf8: &mut [u8],
) -> ExFatResult<usize, D, SIZE>
where
    D: BlockDevice<SIZE>,
{
    let mut cursor = 0;
    for ch in decode_utf16(utf16.iter().copied()) {
        let ch = ch.map_err(|_| ExFatError::InvalidUtf16String {
            reason: "file name contains an invalid utf16 character",
        })?;

        let needed = ch.len_utf8();
        if cursor + needed <= utf8.len() {
            ch.encode_utf8(&mut utf8[cursor..cursor + needed]);
            cursor += needed;
        } else {
            return Err(ExFatError::FileNameBufferTooSmall);
        }
    }

    Ok(cursor)
}

pub(crate) fn is_end_of_directory(directory_entry: &[u8; 32]) -> bool {
    // all bytes in the entry must be zero for this to be an end of directory marker
    directory_entry.iter().all(|&x| x == 0)
}

fn try_into<'a, T: From<&'a RawDirEntry>>(
    dir_entry: &'a RawDirEntry,
    entry_type: EntryType,
) -> Option<T> {
    let et: EntryType = dir_entry[0].into();
    if et == entry_type {
        let entry: T = dir_entry.into();
        Some(entry)
    } else {
        None
    }
}

pub(crate) struct DirSetWriter {
    start: Location,
    current: Location,
    checksum: u16,
}

impl DirSetWriter {
    pub fn new(location: Location) -> Self {
        Self {
            start: location,
            current: location,
            checksum: 0,
        }
    }

    #[bisync]
    pub async fn add<D, const SIZE: usize, const N: usize>(
        &mut self,
        fs: &mut FileSystem<D, SIZE, N>,
        touched: &mut impl Touched,
        dir_entry: &[u8; RAW_ENTRY_LEN],
        is_file_dir: bool,
    ) -> ExFatResult<(), D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        let sector_id = self.current.sector_id;
        let offset = self.current.dir_entry_offset * RAW_ENTRY_LEN;

        let slot = fs.data_blocks.read_mut(sector_id, &mut fs.dev).await?;

        slot.as_mut_slice()[offset..offset + RAW_ENTRY_LEN].copy_from_slice(dir_entry);
        touched.insert(TouchedSector::new(TouchedKind::Dir, sector_id));

        self.next_dir_entry_location::<SIZE>();
        self.calc_checksum(dir_entry, is_file_dir);

        Ok(())
    }

    pub fn add_no_write<const SIZE: usize>(&mut self, dir_entry: &RawDirEntry, is_file_dir: bool) {
        self.next_dir_entry_location::<SIZE>();
        self.calc_checksum(dir_entry, is_file_dir);
    }

    fn next_dir_entry_location<const SIZE: usize>(&mut self) {
        self.current.dir_entry_offset += 1;
        if self.current.dir_entry_offset == SIZE / RAW_ENTRY_LEN {
            // this assumes the set does not cross a cluster boundary (sectors within a cluster are contiguous)
            // TODO: fix this when this crate adds the ability to create multi cluster directories
            self.current.sector_id += 1;
            self.current.dir_entry_offset = 0;
        }
    }

    /// calculates the checksum for a file directory set
    fn calc_checksum(&mut self, raw: &RawDirEntry, is_file_dir: bool) {
        for (byte_index, &b) in raw.iter().enumerate() {
            if is_file_dir && (byte_index == 2 || byte_index == 3) {
                continue;
            }

            self.checksum = self.checksum.rotate_right(1).wrapping_add(b as u16);
        }
    }

    #[bisync]
    pub async fn finish<D, const SIZE: usize, const N: usize>(
        self,
        fs: &mut FileSystem<D, SIZE, N>,
        touched: &mut impl Touched,
        file_dir: &RawDirEntry,
    ) -> ExFatResult<(), D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        let sector_id = self.start.sector_id;
        let offset = self.start.dir_entry_offset * RAW_ENTRY_LEN;

        let slot = fs.data_blocks.read_mut(sector_id, &mut fs.dev).await?;
        let slice = slot.as_mut_slice();
        slice[offset..offset + RAW_ENTRY_LEN].copy_from_slice(file_dir);
        slice[offset + 2..offset + 4].copy_from_slice(&self.checksum.to_le_bytes());
        touched.insert(TouchedSector::new(TouchedKind::Dir, sector_id));
        Ok(())
    }
}

#[allow(unused)]
#[cfg(test)]
mod tests {

    use crate::test_utils::{BLOCK_SIZE, DummyBlockDevice};

    use super::super::only_sync;
    use super::*;

    fn assert_checksum_matches(entry_set: &[u8]) {
        assert!(entry_set.len().is_multiple_of(RAW_ENTRY_LEN));
        let mut writer = DirSetWriter::new(Location::new(0, 0));

        for (i, dir_entry) in entry_set.as_chunks::<RAW_ENTRY_LEN>().0.iter().enumerate() {
            writer.add_no_write::<512>(dir_entry, i == 0);
        }

        let stored = u16::from_le_bytes([entry_set[2], entry_set[3]]);
        assert_eq!(writer.checksum, stored);
    }

    #[rustfmt::skip]
    const ENTRY_SET_HELLO_TXT: [u8; 96] = [
        133, 2, 240, 119, 32, 0, 0, 0, 0, 107, 16, 93, 15, 107, 16, 93, 15, 107, 16, 93, 183, 107, 128, 128, 128, 0, 0, 0, 0, 0, 0, 0,
        192, 3, 0, 9, 70, 48, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0,
        193, 0, 72, 0, 101, 0, 108, 0, 108, 0, 111, 0, 46, 0, 84, 0, 88, 0, 84, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    #[rustfmt::skip]
    const ENTRY_SET_EMOJI: [u8; 256] = [
        133, 7, 145, 42, 32, 0, 0, 0, 60, 107, 16, 93, 60, 107, 16, 93, 60, 107, 16, 93, 21, 21, 128, 128, 128, 0, 0, 0, 0, 0, 0, 0,
        192, 3, 0, 82, 177, 114, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
        193, 0, 84, 0, 104, 0, 105, 0, 115, 0, 32, 0, 105, 0, 115, 0, 32, 0, 97, 0, 32, 0, 118, 0, 101, 0, 114, 0, 121, 0, 32, 0,
        193, 0, 108, 0, 111, 0, 110, 0, 103, 0, 32, 0, 102, 0, 105, 0, 108, 0, 101, 0, 32, 0, 110, 0, 97, 0, 109, 0, 101, 0, 32, 0,
        193, 0, 98, 0, 117, 0, 116, 0, 32, 0, 111, 0, 107, 0, 32, 0, 49, 0, 50, 0, 51, 0, 32, 0, 45, 0, 32, 0, 116, 0, 111, 0,
        193, 0, 32, 0, 117, 0, 115, 0, 101, 0, 32, 0, 97, 0, 115, 0, 32, 0, 97, 0, 110, 0, 32, 0, 101, 0, 120, 0, 102, 0, 97, 0,
        193, 0, 116, 0, 32, 0, 110, 0, 97, 0, 109, 0, 101, 0, 32, 0, 62, 216, 128, 221, 32, 0, 115, 0, 111, 0, 32, 0, 116, 0, 104, 0,
        193, 0, 101, 0, 114, 0, 101, 0, 46, 0, 84, 0, 120, 0, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 
    ];

    #[rustfmt::skip]
    const ENTRY_SET_254_CHAR_NAME: [u8; 608] = [
        133, 18, 52, 185, 32, 0, 0, 0, 203, 107, 16, 93, 203, 107, 16, 93, 203, 107, 16, 93, 35, 35, 128, 128, 128, 0, 0, 0, 0, 0, 0, 0,
        192, 3, 0, 254, 142, 226, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
        193, 0, 72, 0, 101, 0, 114, 0, 101, 0, 32, 0, 105, 0, 115, 0, 32, 0, 97, 0, 110, 0, 32, 0, 101, 0, 120, 0, 97, 0, 109, 0,
        193, 0, 112, 0, 108, 0, 101, 0, 32, 0, 111, 0, 102, 0, 32, 0, 97, 0, 110, 0, 32, 0, 101, 0, 120, 0, 116, 0, 114, 0, 101, 0,
        193, 0, 109, 0, 101, 0, 108, 0, 121, 0, 32, 0, 108, 0, 111, 0, 110, 0, 103, 0, 32, 0, 102, 0, 105, 0, 108, 0, 101, 0, 32, 0,
        193, 0, 110, 0, 97, 0, 109, 0, 101, 0, 32, 0, 98, 0, 117, 0, 116, 0, 32, 0, 105, 0, 116, 0, 32, 0, 115, 0, 104, 0, 111, 0,
        193, 0, 117, 0, 108, 0, 100, 0, 32, 0, 115, 0, 116, 0, 105, 0, 108, 0, 108, 0, 32, 0, 98, 0, 101, 0, 32, 0, 99, 0, 111, 0,
        193, 0, 109, 0, 112, 0, 97, 0, 116, 0, 105, 0, 98, 0, 108, 0, 101, 0, 32, 0, 119, 0, 105, 0, 116, 0, 104, 0, 32, 0, 116, 0,
        193, 0, 104, 0, 101, 0, 32, 0, 101, 0, 120, 0, 102, 0, 97, 0, 116, 0, 32, 0, 102, 0, 105, 0, 108, 0, 101, 0, 32, 0, 83, 0,
        193, 0, 89, 0, 83, 0, 84, 0, 69, 0, 77, 0, 32, 0, 101, 0, 118, 0, 101, 0, 110, 0, 32, 0, 116, 0, 104, 0, 111, 0, 117, 0,
        193, 0, 103, 0, 104, 0, 32, 0, 110, 0, 111, 0, 98, 0, 111, 0, 100, 0, 121, 0, 32, 0, 119, 0, 111, 0, 117, 0, 108, 0, 100, 0,
        193, 0, 32, 0, 69, 0, 86, 0, 69, 0, 82, 0, 89, 0, 32, 0, 117, 0, 115, 0, 101, 0, 32, 0, 97, 0, 32, 0, 102, 0, 105, 0,
        193, 0, 108, 0, 101, 0, 110, 0, 97, 0, 109, 0, 101, 0, 32, 0, 108, 0, 105, 0, 107, 0, 101, 0, 32, 0, 116, 0, 104, 0, 105, 0,
        193, 0, 115, 0, 32, 0, 114, 0, 105, 0, 103, 0, 104, 0, 116, 0, 46, 0, 32, 0, 65, 0, 109, 0, 32, 0, 73, 0, 32, 0, 114, 0,
        193, 0, 105, 0, 103, 0, 104, 0, 116, 0, 46, 0, 32, 0, 77, 0, 97, 0, 121, 0, 98, 0, 101, 0, 32, 0, 73, 0, 109, 0, 32, 0,
        193, 0, 119, 0, 114, 0, 111, 0, 110, 0, 103, 0, 32, 0, 45, 0, 32, 0, 109, 0, 97, 0, 121, 0, 98, 0, 101, 0, 32, 0, 121, 0,
        193, 0, 111, 0, 117, 0, 32, 0, 103, 0, 101, 0, 116, 0, 32, 0, 116, 0, 104, 0, 111, 0, 115, 0, 101, 0, 32, 0, 116, 0, 104, 0,
        193, 0, 97, 0, 116, 0, 32, 0, 100, 0, 111, 0, 110, 0, 116, 0, 32, 0, 101, 0, 118, 0, 101, 0, 110, 0, 32, 0, 117, 0, 115, 0,
        193, 0, 101, 0, 32, 0, 97, 0, 110, 0, 32, 0, 101, 0, 120, 0, 116, 0, 101, 0, 110, 0, 115, 0, 105, 0, 111, 0, 110, 0, 0, 0,
    ];

    #[only_sync]
    #[test]
    fn checksum_matches_real_volume() {
        assert_checksum_matches(&ENTRY_SET_HELLO_TXT);
        assert_checksum_matches(&ENTRY_SET_EMOJI);
        assert_checksum_matches(&ENTRY_SET_254_CHAR_NAME);
    }

    #[only_sync]
    #[test]
    fn checksum_skips_its_own_field() {
        let mut changed = ENTRY_SET_HELLO_TXT;
        changed[2] = 0xDE;
        changed[3] = 0xAD;
        let mut writer = DirSetWriter::new(Location::new(0, 0));

        for (i, dir_entry) in changed.as_chunks::<RAW_ENTRY_LEN>().0.iter().enumerate() {
            writer.add_no_write::<512>(dir_entry, i == 0);
        }

        assert_eq!(writer.checksum, 0x77F0);
    }

    #[only_sync]
    #[test]
    fn checksum_detects_corruption() {
        let mut changed = ENTRY_SET_HELLO_TXT;
        changed[40] ^= 0x01;
        let mut writer = DirSetWriter::new(Location::new(0, 0));

        for (i, dir_entry) in changed.as_chunks::<RAW_ENTRY_LEN>().0.iter().enumerate() {
            writer.add_no_write::<512>(dir_entry, i == 0);
        }

        assert_ne!(writer.checksum, 0x77F0);
    }
}

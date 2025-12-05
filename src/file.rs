use alloc::{string::String, vec, vec::Vec};
use core::str::from_utf8;

use crate::{
    directory_entry::{
        DirectoryEntryChain, FileAttributes, FileNameDirEntry, GeneralSecondaryFlags, Location,
        StreamExtensionDirEntry, next_file_dir_entry,
    },
    error::ExFatError,
    fat::next_cluster_in_fat_chain,
    file_system::FileSystemDetails,
    io::{BLOCK_SIZE, BlockDevice},
    upcase_table::UpcaseTable,
    utils::{calc_dir_entry_set_len, encode_utf16_and_hash},
};

#[derive(Debug)]
pub struct FileDetails {
    pub first_cluster: u32,
    pub data_length: u64,
    pub valid_data_length: u64,
    pub attributes: FileAttributes,
    #[allow(dead_code)]
    pub name: String,
    pub location: Location,
    pub flags: GeneralSecondaryFlags,
}

impl FileDetails {
    pub fn get_num_clusters(&self, bytes_per_cluster: u32) -> usize {
        self.data_length.div_ceil(bytes_per_cluster as u64) as usize
    }
}
pub struct ReadOnlyFile {
    fs: FileSystemDetails,
    no_fat_chain: bool,
    current_cluster: u32,
    cluster_offset: u32,
    valid_data_length: u64,
    bytes_read: u64,
}
impl ReadOnlyFile {
    pub fn new(fs: &FileSystemDetails, file_details: &FileDetails) -> Self {
        Self {
            fs: fs.clone(),
            current_cluster: file_details.first_cluster,
            cluster_offset: 0,
            valid_data_length: file_details.valid_data_length,
            bytes_read: 0,
            no_fat_chain: file_details
                .flags
                .contains(GeneralSecondaryFlags::NoFatChain),
        }
    }

    // we assume that we have already checked for the end of file condition
    // if not then the fat chain will return an error variant for end of fat chain.
    // worse, if there is no fat chain then the current cluster will be set to data not part of this file
    async fn get_current_or_next_cluster(
        &mut self,
        io: &mut impl BlockDevice,
    ) -> Result<(u32, u32), ExFatError> {
        let remainder_in_cluster = (self.fs.cluster_length - self.cluster_offset) as u64;
        if remainder_in_cluster == 0 {
            if self.no_fat_chain {
                // no fat chain so all clusters are consecutive for this file
                self.current_cluster += 1;
                self.cluster_offset = 0;
            } else if let Some(cluster_id) =
                next_cluster_in_fat_chain(io, self.fs.fat_offset, self.current_cluster).await?
            {
                self.current_cluster = cluster_id;
                self.cluster_offset = 0;
            } else {
                return Err(ExFatError::EndOfFatChain);
            }
        }
        Ok((self.current_cluster, self.cluster_offset))
    }

    fn move_file_cursor_by(&mut self, num_bytes: usize) {
        self.bytes_read += num_bytes as u64;
        self.cluster_offset += num_bytes as u32;
    }

    pub async fn read(
        &mut self,
        io: &mut impl BlockDevice,
        buf: &mut [u8],
    ) -> Result<Option<usize>, ExFatError> {
        let remainder_in_file = self.valid_data_length - self.bytes_read;

        // check for end of file
        if remainder_in_file == 0 {
            return Ok(None);
        }

        // skip to the next cluster if required
        let (cluster_id, cluster_offset) = self.get_current_or_next_cluster(io).await?;

        let start_sector_id = self.fs.get_heap_sector_id(cluster_id)?;
        let sector_id = start_sector_id + cluster_offset / BLOCK_SIZE as u32;
        let sector_offset = cluster_offset as usize % BLOCK_SIZE;
        let remainder_in_sector = BLOCK_SIZE - sector_offset;

        // calculate max num bytes we can read
        let num_bytes = (remainder_in_sector as u64)
            .min(remainder_in_file)
            .min(buf.len() as u64) as usize;

        // read a single sector and copy the bytes into the user supplied buffer
        let sector_buf = io.read_sector(sector_id).await?;
        buf[..num_bytes].copy_from_slice(&sector_buf[sector_offset..sector_offset + num_bytes]);

        // update file read cursor position
        self.move_file_cursor_by(num_bytes);

        Ok(Some(num_bytes))
    }

    pub async fn read_to_end(&mut self, io: &mut impl BlockDevice) -> Result<Vec<u8>, ExFatError> {
        let mut buf = vec![0; BLOCK_SIZE];
        let mut contents = Vec::with_capacity(self.valid_data_length as usize);

        while let Some(len) = self.read(io, &mut buf).await? {
            contents.extend_from_slice(&buf[..len]);
        }

        Ok(contents)
    }

    pub async fn read_to_string(
        &mut self,
        io: &mut impl BlockDevice,
    ) -> Result<String, ExFatError> {
        // because multi byte characters may cross sector boundaries
        // I recon its safer to read the entire file into a buffer before decoding it
        let contents = self.read_to_end(io).await?;
        let decoded = from_utf8(&contents)?.into();
        Ok(decoded)
    }
}

pub async fn next_file_entry(
    io: &mut impl BlockDevice,
    entries: &mut DirectoryEntryChain,
    filter: &impl DirectoryEntryFilter,
) -> Result<Option<FileDetails>, ExFatError> {
    'outer: loop {
        if let Some((file_dir_entry, location)) = next_file_dir_entry(io, entries).await? {
            let is_directory = file_dir_entry
                .file_attributes
                .contains(FileAttributes::Directory);
            if let Some((stream_entry, _location)) = entries.next(io).await? {
                // TODO: check entry type
                let stream_entry: StreamExtensionDirEntry = stream_entry.into();
                if !filter.hash(stream_entry.name_hash, is_directory) {
                    continue 'outer;
                }

                // read the entire file_name
                let name_length = stream_entry.name_length as usize;
                let mut file_name: Vec<u16> = Vec::with_capacity(name_length);
                'inner: loop {
                    if let Some((file_name_entry, _location)) = entries.next(io).await? {
                        // TODO: check entry type
                        let file_name_entry: FileNameDirEntry = file_name_entry.into();
                        let len =
                            (name_length - file_name.len()).min(file_name_entry.file_name.len());
                        file_name.extend_from_slice(&file_name_entry.file_name[..len]);
                        if file_name.len() == name_length {
                            break 'inner;
                        }
                    } else {
                        return Ok(None);
                    }
                }

                if !filter.file_name(&file_name) {
                    continue 'outer;
                }

                let name = decode_utf16(file_name)?;
                let file_details = FileDetails {
                    attributes: file_dir_entry.file_attributes,
                    data_length: stream_entry.data_length,
                    valid_data_length: stream_entry.valid_data_length,
                    first_cluster: stream_entry.first_cluster,
                    name,
                    location,
                    flags: stream_entry.general_secondary_flags,
                };
                return Ok(Some(file_details));
            } else {
                return Ok(None);
            }
        } else {
            return Ok(None);
        }
    }
}

fn decode_utf16(buf: Vec<u16>) -> Result<String, ExFatError> {
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

pub trait DirectoryEntryFilter {
    fn hash(&self, file_name_hash: u16, is_directory: bool) -> bool;
    fn file_name(&self, file_name: &[u16]) -> bool;
}

pub struct AllPassFilter {}

impl DirectoryEntryFilter for AllPassFilter {
    fn hash(&self, _file_name_hash: u16, _is_directory: bool) -> bool {
        true
    }

    fn file_name(&self, _file_name: &[u16]) -> bool {
        true
    }
}

pub struct ExactNameFilter<'a> {
    upcase_table: &'a UpcaseTable,
    file_name: Vec<u16>,
    file_name_hash: u16,
    is_directory: bool,
}

impl<'a> ExactNameFilter<'a> {
    pub fn new(file_name_str: &str, upcase_table: &'a UpcaseTable, is_directory: bool) -> Self {
        let (file_name, file_name_hash) = encode_utf16_and_hash(file_name_str, upcase_table);
        Self {
            upcase_table,
            file_name,
            file_name_hash,
            is_directory,
        }
    }
}

impl<'a> DirectoryEntryFilter for ExactNameFilter<'a> {
    fn hash(&self, file_name_hash: u16, is_directory: bool) -> bool {
        self.file_name_hash == file_name_hash && self.is_directory == is_directory
    }

    fn file_name(&self, file_name: &[u16]) -> bool {
        // perform case insensitive name match
        for (left, right) in self.file_name.iter().zip(file_name.iter()) {
            let upcased = self.upcase_table.upcase(*right);
            if *left != upcased {
                // name does not match
                return false;
            }
        }

        true
    }
}

async fn get_leaf_file_entry(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    full_path: &str,
    is_directory: bool,
) -> Result<Option<FileDetails>, ExFatError> {
    let mut splits = full_path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(|c| c.trim())
        .peekable();

    let mut cluster_id = fs.first_cluster_of_root_dir;

    while let Some(part) = splits.next() {
        let is_last = splits.peek().is_none();
        let is_directory_filter = is_directory || !is_last;

        let filter = ExactNameFilter::new(part, upcase_table, is_directory_filter);
        let mut entries = DirectoryEntryChain::new(cluster_id, fs);
        let file_details = next_file_entry(io, &mut entries, &filter).await?;

        match file_details {
            Some(file_details) => {
                if is_last {
                    // file or directory (there might be a directory and a file with the same name but that would have been filtered out above)
                    return Ok(Some(file_details));
                } else {
                    // directory
                    if file_details.attributes.contains(FileAttributes::Directory) {
                        cluster_id = file_details.first_cluster
                    } else {
                        return Ok(None);
                    }
                }
            }
            None => return Ok(None),
        }
    }

    Ok(None)
}

fn is_root_directory(path: &str) -> bool {
    let mut splits = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(|c| c.trim())
        .peekable();

    splits.peek().is_none()
}

pub async fn directory_list(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    full_path: &str,
) -> Result<DirectoryIterator, ExFatError> {
    let cluster_id = if is_root_directory(full_path) {
        fs.first_cluster_of_root_dir
    } else {
        match get_leaf_file_entry(io, fs, upcase_table, full_path, true).await? {
            Some(file_details) => {
                if file_details.attributes.contains(FileAttributes::Directory) {
                    file_details.first_cluster
                } else {
                    return Err(ExFatError::DirectoryNotFound);
                }
            }
            None => return Err(ExFatError::DirectoryNotFound),
        }
    };

    let entries = DirectoryEntryChain::new(cluster_id, fs);
    Ok(DirectoryIterator { entries })
}

pub struct DirectoryIterator {
    entries: DirectoryEntryChain,
}

impl DirectoryIterator {
    pub async fn next(
        &mut self,
        io: &mut impl BlockDevice,
    ) -> Result<Option<FileDetails>, ExFatError> {
        let filter = AllPassFilter {};
        next_file_entry(io, &mut self.entries, &filter).await
    }
}

pub async fn find_file(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    full_path: &str,
) -> Result<FileDetails, ExFatError> {
    let file_details = find_file_inner(io, fs, upcase_table, full_path).await?;
    Ok(file_details)
}

// TODO: figure out visibility (pub or private)
pub async fn find_file_inner(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    full_path: &str,
) -> Result<FileDetails, ExFatError> {
    match get_leaf_file_entry(io, fs, upcase_table, full_path, false).await? {
        Some(file_details) => {
            if file_details.attributes.contains(FileAttributes::Archive) {
                Ok(file_details)
            } else {
                Err(ExFatError::FileNotFound)
            }
        }
        None => Err(ExFatError::FileNotFound),
    }
}

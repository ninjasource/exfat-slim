use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::str::from_utf8;

use super::{
    allocation_bitmap::AllocationBitmap,
    bisync,
    directory_entry::{
        DirectoryEntryChain, FileAttributes, FileNameDirEntry, GeneralSecondaryFlags, Location,
        StreamExtensionDirEntry, next_file_dir_entry,
    },
    error::ExFatError,
    fat::next_cluster_in_fat_chain,
    file_system::FileSystem,
    file_system::FileSystemDetails,
    io::{BLOCK_SIZE, BlockDevice},
    upcase_table::UpcaseTable,
    utils::{calc_dir_entry_set_len, encode_utf16_upcase_and_hash},
};

#[derive(Clone, Debug)]
pub struct OpenOptions {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub truncate: bool,
    pub create: bool,
    pub create_new: bool,
}

impl OpenOptions {
    pub fn new() -> Self {
        OpenOptions {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
        }
    }

    /// Set option for read access
    ///
    /// If read it true the file should be readable if opened
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    /// Set option for write access
    ///
    /// If write is true the file should be writable if opened
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    /// Sets the option for append mode
    ///
    /// If append is true then writes will append to a file instead of overwriting its contents
    /// Setting `.write(true).append(true)` has the same affect as only setting `.append(true)`
    /// This option does not create a file if it does not exist, use create or create_new for that
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    /// Sets the option to truncate the previous file
    ///
    /// If truncate is true, opening the file will truncate the file length to 0 if it already exists.
    /// The file must be opened with `.write(true)` for this to work.
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    /// Sets the option to create a new file or simply open it if it already exists
    ///
    /// In order for the file to be created either `.write(true)` or `.append(true)` must be used.
    /// Calling `.create()` without `.write()` or `append()` will return an error on open
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    /// Sets the option to create a new file and failing if it already exists
    ///
    /// In order for the file to be created either `.write(true)` or `.append(true)` must be used.
    /// If true `.create()` and `.truncate()` are ignored
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    pub fn build(&self) -> Self {
        self.clone()
    }
}

#[derive(Debug, Clone)]
pub struct FileDetails {
    pub first_cluster: u32,
    pub data_length: u64,
    pub valid_data_length: u64,
    pub attributes: FileAttributes,
    #[allow(dead_code)]
    pub name: String,
    pub location: Location,
    pub flags: GeneralSecondaryFlags,
    pub secondary_count: u8,
}

impl FileDetails {
    pub fn get_num_clusters(&self, bytes_per_cluster: u32) -> usize {
        self.data_length.div_ceil(bytes_per_cluster as u64) as usize
    }
}

pub struct File {
    fs: FileSystemDetails,
    current_cluster: u32,
    cluster_offset: u32, // in bytes
    cursor: u64,
    pub data_length: u64,
    pub valid_data_length: u64,
    pub attributes: FileAttributes,
    alloc_bitmap: AllocationBitmap,
    flags: GeneralSecondaryFlags,
    open_options: OpenOptions,
}

struct Allocation {
    start_cluster_index: u32,
    fs: FileSystemDetails,
    sector_index: usize,
    cluster_ids: Vec<u32>,
}

impl Allocation {
    #[bisync]
    async fn new(
        io: &mut impl BlockDevice,
        file: &File,
        buf_len: usize,
    ) -> Result<Self, ExFatError> {
        unimplemented!()
    }

    fn next_sector_id(&mut self) -> Result<u32, ExFatError> {
        let cluster_index = self.sector_index / self.fs.sectors_per_cluster as usize;
        if cluster_index >= self.cluster_ids.len() {
            return Err(ExFatError::DiskFull);
        }

        let cluster_id = self.cluster_ids[cluster_index];
        let start_sector_id = self.fs.get_heap_sector_id(cluster_id)?;
        let sector_id = start_sector_id + self.sector_index as u32;
        self.sector_index += 1;
        Ok(sector_id)
    }
}

impl File {
    fn next_sector_id(&mut self, current_cluster_index: &mut usize, cluster_ids: &[u32]) -> u32 {
        0
    }
    fn get_current_sector_id(&self) -> Result<(u32, usize), ExFatError> {
        let cluster_offset_bytes = self.cursor % self.fs.cluster_length as u64;
        let start_sector_id = self.fs.get_heap_sector_id(self.current_cluster)?;
        let sector_id =
            start_sector_id + cluster_offset_bytes as u32 / self.fs.sectors_per_cluster as u32;
        let offset = cluster_offset_bytes as usize % self.fs.sectors_per_cluster as usize;
        Ok((sector_id, offset))
    }

    pub fn new(
        file_system: &FileSystem,
        file_details: &FileDetails,
        open_options: &OpenOptions,
    ) -> Self {
        let cursor = if open_options.append {
            file_details.valid_data_length
        } else {
            0
        };
        Self {
            fs: file_system.fs.clone(),
            current_cluster: file_details.first_cluster,
            cluster_offset: 0,
            cursor,
            alloc_bitmap: file_system.alloc_bitmap.clone(),
            data_length: file_details.data_length,
            flags: file_details.flags,
            valid_data_length: file_details.valid_data_length,
            attributes: file_details.attributes,
            open_options: open_options.clone(),
        }
    }

    #[bisync]
    pub async fn write(&mut self, io: &mut impl BlockDevice, buf: &[u8]) -> Result<(), ExFatError> {
        if !self.open_options.write {
            return Err(ExFatError::WriteNotEnabled);
        }

        let mut allocation = Allocation::new(io, self, buf.len()).await?;

        let first_sector_id_in_cluster = self.fs.get_heap_sector_id(self.current_cluster)?;
        let mut sector_id = first_sector_id_in_cluster + self.cluster_offset;

        //let cluster_ids = self.get_or_allocate_clusters(io, buf.len()).await?;
        let cluster_ids = vec![];
        let mut start_index = (self.cursor % BLOCK_SIZE as u64) as usize;

        let (sector_id, offset) = self.get_current_sector_id()?;

        // if the write does not start on a block boundary
        // we need to read the existing sector and add in the bit we want to write
        // for max efficiency the user should always write in block size chunks
        if offset > 0 {
            let end_index = BLOCK_SIZE.min(buf.len());
            let block = io.read_sector(sector_id).await?;
            let mut temp = [0u8; BLOCK_SIZE];
            temp[..start_index].copy_from_slice(&block[..start_index]);
            temp[start_index..].copy_from_slice(&buf[start_index..end_index]);
            io.write_sector(sector_id, &temp).await?;
            start_index = end_index;

            if sector_id % self.fs.sectors_per_cluster as u32 == 0 {
                self.current_cluster = cluster_ids[1];
                self.cluster_offset = 0;
            }
        }

        /*
        if start_index > 0 {
            let end_index = BLOCK_SIZE.min(buf.len());
            let block = io.read_sector(sector_id).await?;
            let mut temp = [0u8; BLOCK_SIZE];
            temp[..start_index].copy_from_slice(&block[..start_index]);
            temp[start_index..].copy_from_slice(&buf[start_index..end_index]);
            io.write_sector(sector_id, &temp).await?;
            start_index = end_index;
            sector_id += 1;
            if sector_id % self.fs.sectors_per_cluster as u32 == 0 {
                self.current_cluster = cluster_ids[1];
                self.cluster_offset = 0;
            }
        }*/

        let (blocks, remainder) = buf[start_index..].as_chunks::<BLOCK_SIZE>();
        for block in blocks {
            let first_sector_id_in_cluster = self.fs.get_heap_sector_id(self.current_cluster)?;
            //  let sector_id = first_sector_id_in_cluster +
        }

        for cluster_id in cluster_ids {
            let first_sector_id_in_cluster = self.fs.get_heap_sector_id(self.current_cluster)?;

            for cluster_offset in self.cluster_offset..self.fs.sectors_per_cluster as u32 {
                let sector_id = first_sector_id_in_cluster + cluster_offset;

                // if file cursor is a multiple of block size we dont need to worry about partial writes to a block
                // if self.cursor % BLOCK_SIZE == 0 {}
            }

            if self.cluster_offset as usize % BLOCK_SIZE == 0 {
                // no need to buffer writes here because they can be flushed immediately
                let sector_id = self.fs.get_heap_sector_id(self.current_cluster)?
                    + self.cluster_offset / BLOCK_SIZE as u32;

                let (blocks, remainder) = buf.as_chunks::<BLOCK_SIZE>();
                for block in blocks {
                    io.write_sector(sector_id, block).await?;
                    self.cluster_offset += BLOCK_SIZE as u32;

                    if self.cluster_offset >= self.fs.cluster_length {
                        // TODO: fix this clearly incorrect assumption that a growing file is no-fat-chain
                        self.current_cluster += 1;
                        self.cluster_offset = 0;
                    }
                }

                if remainder.len() > 0 {
                    unimplemented!("length must be multiple of BLOCK_SIZE")
                }
            } else {
                // in order to support this we need to read the sector and copy in the buf bytes
                unimplemented!("unalligned writes not supported")
            }
        }

        Ok(())
    }

    // we assume that we have already checked for the end of file condition
    // if not then the fat chain will return an error variant for end of fat chain.
    // worse, if there is no fat chain then the current cluster will be set to data not part of this file
    #[bisync]
    async fn get_current_or_next_cluster(
        &mut self,
        io: &mut impl BlockDevice,
    ) -> Result<(u32, u32), ExFatError> {
        let remainder_in_cluster = (self.fs.cluster_length - self.cluster_offset) as u64;
        if remainder_in_cluster == 0 {
            if self.flags.contains(GeneralSecondaryFlags::NoFatChain) {
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
        self.cursor += num_bytes as u64;
        self.cluster_offset += num_bytes as u32;
    }

    #[bisync]
    pub async fn read(
        &mut self,
        io: &mut impl BlockDevice,
        buf: &mut [u8],
    ) -> Result<Option<usize>, ExFatError> {
        if !self.open_options.read {
            return Err(ExFatError::ReadNotEnabled);
        }

        let remainder_in_file = self.valid_data_length - self.cursor;

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

    /// Read all bytes from file into the buffer
    /// This behaves the same way the Rust std library equivalent function works
    /// If you only want the buf to contain file bytes then pass in an empty buf (length zero)
    /// If you pass in a non zero length buf the file bytes will be appended onto the end of the buf
    /// If you want to pass in preallocated memory then you are free to set the capacity of the buf passed in
    /// and the file will be copied from position 0 in the buf (if it is length 0)
    #[bisync]
    pub async fn read_to_end(
        &mut self,
        io: &mut impl BlockDevice,
        buf: &mut Vec<u8>,
    ) -> Result<usize, ExFatError> {
        let len = self.valid_data_length as usize;

        // fill empty space with zeros
        buf.resize(buf.len() + len, 0);

        // reading in block size chunks from position 0 is the most efficient way to get data off the disk in one go
        // we can ignore the len returned from the read operation as a result
        let (blocks, remainder) = buf.as_chunks_mut::<BLOCK_SIZE>();
        for block in blocks {
            self.read(io, block.as_mut_slice()).await?;
        }
        self.read(io, remainder).await?;

        Ok(len)
    }

    #[bisync]
    pub async fn read_to_string(
        &mut self,
        io: &mut impl BlockDevice,
    ) -> Result<String, ExFatError> {
        // because multi byte characters may cross sector boundaries
        // I recon its safer to read the entire file into a buffer before decoding it
        let mut buf = Vec::new();
        let len = self.read_to_end(io, &mut buf).await?;
        let decoded = from_utf8(&buf[..len])?.into();
        Ok(decoded)
    }
}

#[bisync]
pub async fn next_file_entry(
    io: &mut impl BlockDevice,
    entries: &mut DirectoryEntryChain,
    filter: &impl DirectoryEntryFilter,
) -> Result<Option<FileDetails>, ExFatError> {
    'outer: loop {
        if let Some((file_dir_entry, location)) = next_file_dir_entry(io, entries).await? {
            if let Some((stream_entry, _location)) = entries.next(io).await? {
                // TODO: check entry type
                let stream_entry: StreamExtensionDirEntry = stream_entry.into();
                if !filter.hash(stream_entry.name_hash, file_dir_entry.file_attributes) {
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
                    secondary_count: file_dir_entry.secondary_count,
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
    fn hash(&self, file_name_hash: u16, file_attributes: FileAttributes) -> bool;
    fn file_name(&self, file_name: &[u16]) -> bool;
}

pub struct AllPassFilter {}

impl DirectoryEntryFilter for AllPassFilter {
    fn hash(&self, _file_name_hash: u16, _file_attributes: FileAttributes) -> bool {
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
    file_attributes: Option<FileAttributes>,
}

impl<'a> ExactNameFilter<'a> {
    pub fn new(
        file_name_str: &str,
        upcase_table: &'a UpcaseTable,
        file_attributes: Option<FileAttributes>,
    ) -> Self {
        let (file_name, file_name_hash) = encode_utf16_upcase_and_hash(file_name_str, upcase_table);
        Self {
            upcase_table,
            file_name,
            file_name_hash,
            file_attributes,
        }
    }
}

impl<'a> DirectoryEntryFilter for ExactNameFilter<'a> {
    fn hash(&self, file_name_hash: u16, file_attributes: FileAttributes) -> bool {
        match self.file_attributes {
            Some(attributes) => {
                self.file_name_hash == file_name_hash && file_attributes.contains(attributes)
            }
            None => self.file_name_hash == file_name_hash,
        }
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

#[bisync]
async fn get_leaf_file_entry(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    full_path: &str,
    file_attributes: Option<FileAttributes>,
) -> Result<Option<FileDetails>, ExFatError> {
    let mut splits = full_path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(|c| c.trim())
        .peekable();

    let mut cluster_id = fs.first_cluster_of_root_dir;

    while let Some(part) = splits.next() {
        let is_last = splits.peek().is_none();
        let attributes = if is_last {
            file_attributes
        } else {
            Some(FileAttributes::Directory)
        };

        //let is_directory_filter = is_directory || !is_last;

        let filter = ExactNameFilter::new(part, upcase_table, attributes);
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

#[bisync]
pub async fn directory_list(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    full_path: &str,
) -> Result<DirectoryIterator, ExFatError> {
    let cluster_id = if is_root_directory(full_path) {
        fs.first_cluster_of_root_dir
    } else {
        match get_leaf_file_entry(
            io,
            fs,
            upcase_table,
            full_path,
            Some(FileAttributes::Directory),
        )
        .await?
        {
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
    #[bisync]
    pub async fn next(
        &mut self,
        io: &mut impl BlockDevice,
    ) -> Result<Option<FileDetails>, ExFatError> {
        let filter = AllPassFilter {};
        next_file_entry(io, &mut self.entries, &filter).await
    }
}

#[bisync]
pub async fn find_file_or_directory(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    full_path: &str,
    file_attributes: Option<FileAttributes>,
) -> Result<FileDetails, ExFatError> {
    let file_details = find_file_inner(io, fs, upcase_table, full_path, file_attributes).await?;
    Ok(file_details)
}

// TODO: figure out visibility (pub or private)
#[bisync]
pub async fn find_file_inner(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    full_path: &str,
    file_attributes: Option<FileAttributes>,
) -> Result<FileDetails, ExFatError> {
    match get_leaf_file_entry(io, fs, upcase_table, full_path, file_attributes).await? {
        Some(file_details) => Ok(file_details),
        None => Err(ExFatError::FileNotFound),
    }
}

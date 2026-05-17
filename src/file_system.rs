use aligned::Aligned;

#[cfg(feature = "alloc")]
use alloc::{string::String, vec::Vec};

use super::{
    BlockDevice,
    allocation::{AllocationBitmap, Allocator, StoredChain},
    bisync,
    boot_sector::BootSector,
    directory::{DirectoryIterator, ExactNameFilter, directory_list, get_leaf_file_entry},
    directory_entry::{
        AllocationBitmapDirEntry, DirSetWriter, DirectoryEntryChain, EntryType, FileAttributes,
        FileDirEntry, FileNameDirEntry, GeneralSecondaryFlags, Location, RAW_ENTRY_LEN,
        RawDirEntry, StreamExtensionDirEntry, UpcaseTableDirEntry, VolumeLabelDirEntry,
        is_end_of_directory,
    },
    error::ExFatError,
    fat::Fat,
    file::{File, FileDetails, FileDirty, NO_CLUSTER_ID, OpenOptions, Touched},
    slot_cache::SlotCache,
    upcase_table::UpcaseTable,
    utils::{Utf16Chunks, calc_dir_entry_set_len, encode_utf16_upcase_and_hash, split_path},
};

pub type ExFatResult<T, D, const SIZE: usize> =
    core::result::Result<T, ExFatError<<D as BlockDevice<SIZE>>::Error>>;

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, Clone)]
pub(crate) struct FileSystemDetails {
    /// volume-relative sector offset of the cluster heap
    pub cluster_heap_offset: u32,

    /// volume-relative sector offset of the first FAT
    pub fat_offset: u32,

    /// number of sectors per cluster (64 is normal for an SD card of 8GB - that equates to 32KB per cluster)
    pub sectors_per_cluster: u8,

    /// number of bytes in a cluster
    pub cluster_length: u32,

    /// first cluster of root directory
    pub first_cluster_of_root_dir: u32,
}

impl FileSystemDetails {
    pub(crate) const fn empty() -> Self {
        Self {
            cluster_heap_offset: 0,
            fat_offset: 0,
            sectors_per_cluster: 64,
            cluster_length: 32768,
            first_cluster_of_root_dir: 0,
        }
    }

    pub(crate) fn new(boot_sector: &BootSector) -> Self {
        let cluster_length =
            boot_sector.bytes_per_sector as u32 * boot_sector.sectors_per_cluster as u32;
        Self {
            cluster_heap_offset: boot_sector.cluster_heap_offset,
            sectors_per_cluster: boot_sector.sectors_per_cluster,
            cluster_length,
            fat_offset: boot_sector.fat_offset,
            first_cluster_of_root_dir: boot_sector.first_cluster_of_root_dir,
        }
    }

    pub(crate) fn get_heap_sector_id<D, const SIZE: usize>(
        &self,
        cluster_id: u32,
    ) -> ExFatResult<u32, D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        if cluster_id < 2 {
            return Err(ExFatError::InvalidClusterId(cluster_id));
        }

        let sector_id =
            self.cluster_heap_offset + (cluster_id - 2) * self.sectors_per_cluster as u32;
        Ok(sector_id)
    }
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug)]
pub struct FileSystem<D, const SIZE: usize, const N: usize>
where
    D: BlockDevice<SIZE>,
{
    pub(crate) dev: D,
    pub(crate) is_mounted: bool,
    pub(crate) fs: FileSystemDetails,
    pub(crate) upcase_table: UpcaseTable,
    pub(crate) allocator: Allocator<D, SIZE, N>,
    pub(crate) fat: Fat<D, SIZE, N>,
    pub(crate) data_blocks: SlotCache<D, SIZE, N>,
}

pub(crate) struct FileSystemMetadata<const SIZE: usize> {
    details: FileSystemDetails,
    upcase_table: UpcaseTable,
    alloc_bitmap: AllocationBitmap<SIZE>,
}

impl<D, const SIZE: usize, const N: usize> FileSystem<D, SIZE, N>
where
    D: BlockDevice<SIZE>,
{
    pub fn new(io: D) -> Self {
        let fs = FileSystemDetails::empty();
        let upcase_table = UpcaseTable::empty();
        let fat = Fat::new();
        let allocator = Allocator::new();
        let data_blocks: SlotCache<D, SIZE, N> = SlotCache::new();

        Self {
            dev: io,
            fs,
            upcase_table,
            is_mounted: false,
            fat,
            allocator,
            data_blocks,
        }
    }

    pub fn into_inner(self) -> D {
        self.dev
    }
}

impl<D, const SIZE: usize, const N: usize> FileSystem<D, SIZE, N>
where
    D: BlockDevice<SIZE>,
{
    /// Calling this is optional as the file system will be automatically mounted upon first use if it is not already mounted
    /// Reads the boot sector using the block device and initializes the file system returning an instance of it
    #[bisync]
    pub async fn mount(&mut self) -> ExFatResult<(), D, SIZE> {
        if self.is_mounted {
            return Ok(());
        }

        let FileSystemMetadata {
            details,
            upcase_table,
            alloc_bitmap,
        } = read_file_system_metadata(&mut self.dev).await?;
        self.fat.start_of_fat_sector = Some(details.fat_offset);
        self.fs = details;
        self.allocator.bitmap.first_sector = self
            .fs
            .get_heap_sector_id::<D, SIZE>(alloc_bitmap.first_cluster)?;
        self.allocator.bitmap.num_sectors = alloc_bitmap.num_sectors;
        self.upcase_table = upcase_table;
        self.is_mounted = true;
        Ok(())
    }

    #[bisync]
    pub async fn open(&mut self, path: &str, options: OpenOptions) -> ExFatResult<File, D, SIZE> {
        self.mount().await?;

        // attempt to get the file details
        let file_details = self
            .find_file_or_directory(path, Some(FileAttributes::Archive))
            .await;

        // get file details or create if required
        let file_details = match file_details {
            Ok(mut file_details) => {
                if options.create_new {
                    return Err(ExFatError::AlreadyExists);
                }

                if options.truncate {
                    self.truncate_file(&mut file_details, 0).await?;
                }

                file_details
            }
            Err(ExFatError::FileNotFound) => {
                if options.create || options.create_new {
                    self.create_file(path).await?
                } else {
                    return Err(ExFatError::FileNotFound);
                }
            }
            Err(e) => return Err(e),
        };

        let chain = self.get_stored_chain(&file_details).await?;
        let cluster_id = if options.append {
            match &chain {
                StoredChain::Empty => file_details.first_cluster,
                StoredChain::Contiguous {
                    first,
                    cluster_count,
                } => first + cluster_count - 1,
                StoredChain::Fat {
                    first: _first,
                    last,
                    cluster_count: _cluster_count,
                } => *last,
            }
        } else {
            file_details.first_cluster
        };

        Ok(File::new(
            &file_details,
            cluster_id,
            self.fs.cluster_length,
            &options,
            chain,
        ))
    }

    /// Returns true if the file or directory exists
    ///
    /// Symbolic link following is not supported
    #[bisync]
    pub async fn exists(&mut self, path: &str) -> ExFatResult<bool, D, SIZE> {
        self.mount().await?;

        match self.find_file_or_directory(path, None).await {
            Ok(_file) => Ok(true),
            Err(ExFatError::FileNotFound) => Ok(false),
            Err(ExFatError::DirectoryNotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Reads the entire contents of the file into a byte vector
    ///
    /// Supports nested paths
    #[cfg(feature = "alloc")]
    #[bisync]
    pub async fn read(&mut self, path: &str) -> ExFatResult<Vec<u8>, D, SIZE> {
        self.mount().await?;
        let options = OpenOptions::new().read(true);
        let mut file = self.open(path, options).await?;
        let mut buf = Vec::new();
        file.read_to_end(self, &mut buf).await?;
        file.close(self).await?;
        Ok(buf)
    }

    /// Reads the entire contents of the file into a string
    ///
    /// Supports nested paths
    #[cfg(feature = "alloc")]
    #[bisync]
    pub async fn read_to_string(&mut self, path: &str) -> ExFatResult<String, D, SIZE> {
        self.mount().await?;
        let options = OpenOptions::new().read(true);
        let mut file = self.open(path, options).await?;
        let s = file.read_to_string(self).await?;
        file.close(self).await?;
        Ok(s)
    }

    /// Returns an iterator over the entries in a directory
    ///
    /// Supports nested paths
    #[bisync]
    pub async fn read_dir(&mut self, path: &str) -> ExFatResult<DirectoryIterator<SIZE>, D, SIZE> {
        self.mount().await?;
        directory_list(self, path).await
    }

    /// Deletes a file
    ///
    /// Returns an error if the file does not exist or something else failed
    #[bisync]
    pub async fn remove_file(&mut self, path: &str) -> ExFatResult<(), D, SIZE> {
        self.mount().await?;
        let file_details = self
            .find_file_inner(path, Some(FileAttributes::Archive))
            .await?;
        self.delete_inner(&file_details).await?;
        Ok(())
    }

    /// Creates a directory recursively
    ///
    /// The dir path can be nested
    #[bisync]
    pub async fn create_directory(&mut self, path: &str) -> ExFatResult<(), D, SIZE> {
        self.mount().await?;
        let mut touched = FileDirty::new();

        // find directory or recursively create it if it does not already exist
        let _cluster_id = self.get_or_create_directory(&mut touched, path).await?;

        touched.flush(self).await?;
        Ok(())
    }

    /// Copies a file from one file to another
    ///
    /// If directories in the to_path do not exist they will be created
    /// File attributes will also be copied but timestamps will be new
    #[bisync]
    pub async fn copy(&mut self, from_path: &str, to_path: &str) -> ExFatResult<(), D, SIZE> {
        self.mount().await?;
        if from_path == to_path {
            return Err(ExFatError::InvalidFileName {
                reason: "cannot copy file to the same exact location",
            });
        }
        let options = OpenOptions::new().read(true);
        let mut file = self.open(from_path, options).await?;
        file.copy_to(self, to_path).await?;
        file.close(self).await?;
        Ok(())
    }

    /// Rename a file or folder (also know as `move`)
    ///
    /// The from_path is the old file or directory path (including all sub directories)
    /// The to_path is the new file or directory path (including all sub directories)
    /// If the file or directory changes parent directories then this is considered to be a file move, otherwise a rename
    /// If the to_path contains directories that don't yet exist they will be created.
    #[bisync]
    pub async fn rename(&mut self, from_path: &str, to_path: &str) -> ExFatResult<(), D, SIZE> {
        self.mount().await?;
        // in exFAT a directory cannot have a directory and file with the same name in it so no need to filter here
        let file_details = self.find_file_inner(from_path, None).await?;
        let mut touched = FileDirty::new();

        let mut dir_set_writer = DirSetWriter::new(file_details.location);
        let dir_entry = [0u8; RAW_ENTRY_LEN];

        for _ in 0..file_details.secondary_count + 1 {
            dir_set_writer
                .add(self, &mut touched, &dir_entry, false)
                .await?;
        }

        dir_set_writer
            .finish_no_checksum(self, &mut touched)
            .await?;

        let (dir_path, file_or_dir_name) = split_path(to_path);

        // find directory or recursively create it if it does not already exist
        let directory_cluster_id = self.get_or_create_directory(&mut touched, dir_path).await?;

        self.create_file_dir_entry_at(
            &mut touched,
            file_or_dir_name,
            directory_cluster_id,
            file_details.first_cluster,
            file_details.attributes,
            file_details.flags,
            file_details.valid_data_length,
            file_details.data_length,
        )
        .await?;

        touched.flush(self).await?;

        Ok(())
    }

    /// Removes an empty directory
    ///
    /// Will return an error if the directory does not exist or is not empty
    #[bisync]
    pub async fn remove_dir(&mut self, path: &str) -> ExFatResult<(), D, SIZE> {
        self.mount().await?;
        let file_details = self
            .find_file_inner(path, Some(FileAttributes::Directory))
            .await?;
        self.delete_inner(&file_details).await?;
        Ok(())
    }

    /// Writes the entire contents to a file.
    ///
    /// The file will be overwritten if it already exists.
    /// This function will create any directories in the path that do not already exist.
    /// Relative paths are not supported.
    #[bisync]
    pub async fn write(
        &mut self,
        path: &str,
        contents: impl AsRef<[u8]>,
    ) -> ExFatResult<(), D, SIZE> {
        self.mount().await?;
        let options = OpenOptions::new().create(true).truncate(true).write(true);
        let mut file = self.open(path, options).await?;
        file.write(self, contents.as_ref()).await?;
        file.close(self).await?;
        Ok(())
    }

    #[bisync]
    pub(crate) async fn get_stored_chain(
        &mut self,
        file_details: &FileDetails,
    ) -> ExFatResult<StoredChain, D, SIZE> {
        let no_fat_chain = file_details
            .flags
            .contains(GeneralSecondaryFlags::NoFatChain);
        let cluster_count = file_details
            .data_length
            .div_ceil(self.fs.cluster_length as u64) as u32;
        let is_empty = file_details.first_cluster == NO_CLUSTER_ID;

        let chain = if is_empty {
            StoredChain::Empty
        } else if no_fat_chain {
            StoredChain::Contiguous {
                first: file_details.first_cluster,
                cluster_count,
            }
        } else {
            let last = self
                .get_cluster_id_at(file_details.data_length, file_details)
                .await?;
            StoredChain::Fat {
                first: file_details.first_cluster,
                last,
                cluster_count,
            }
        };

        Ok(chain)
    }

    #[bisync]
    pub(crate) async fn find_file_or_directory(
        &mut self,
        path: &str,
        file_attributes: Option<FileAttributes>,
    ) -> ExFatResult<FileDetails, D, SIZE> {
        let file_details = self.find_file_inner(path, file_attributes).await?;
        Ok(file_details)
    }

    // TODO: figure out visibility (pub or private)
    #[bisync]
    pub(crate) async fn find_file_inner(
        &mut self,
        path: &str,
        file_attributes: Option<FileAttributes>,
    ) -> ExFatResult<FileDetails, D, SIZE> {
        match get_leaf_file_entry(self, path, file_attributes).await? {
            Some(file_details) => Ok(file_details),
            None => Err(ExFatError::FileNotFound),
        }
    }

    /// Sets a file to length specified and allocates or frees up all the clusters linked to the file
    /// If length is set to zero then only the first cluster in the file will be allocated.
    #[bisync]
    pub(crate) async fn truncate_file(
        &mut self,
        file_details: &mut FileDetails,
        length: u64,
    ) -> ExFatResult<(), D, SIZE> {
        // TODO: support length greater than 0 for preallocated files
        if length > 0 {
            unimplemented!("length greater than 0 not yet supported")
        }
        let mut touched = FileDirty::new();

        let mut chain = DirectoryEntryChain::new_from_file_details(file_details, &self.fs);
        let mut count = 0;
        let mut dir_set_writer = DirSetWriter::new(file_details.location);
        let mut file_dir: Option<RawDirEntry> = None;

        while let Some((dir_entry, _location)) = chain.next(self).await? {
            match count {
                0 => {
                    // file dir
                    dir_set_writer.add_no_write::<SIZE>(dir_entry, true);
                    let mut raw = [0u8; RAW_ENTRY_LEN];
                    raw.copy_from_slice(dir_entry);
                    file_dir = Some(raw);
                }
                1 => {
                    // stream ext
                    let mut stream_ext: StreamExtensionDirEntry = dir_entry.into();
                    stream_ext.data_length = file_details.data_length;
                    stream_ext.valid_data_length = file_details.valid_data_length;
                    dir_set_writer
                        .add(self, &mut touched, &stream_ext.serialize(), false)
                        .await?;
                }
                _ => {
                    // file name
                    dir_set_writer.add_no_write::<SIZE>(dir_entry, false);
                }
            }

            count += 1;
        }

        if let Some(file_dir) = file_dir {
            dir_set_writer.finish(self, &mut touched, &file_dir).await?;
        }

        // set file length to 0
        file_details.data_length = length;
        file_details.valid_data_length = file_details.valid_data_length.min(length);

        // mark all clusters as free
        let chain = self.get_stored_chain(file_details).await?;
        self.allocator
            .free(&mut self.dev, &mut touched, &mut self.fat, &chain)
            .await?;

        touched.flush(self).await?;

        Ok(())
    }

    #[bisync]
    pub(crate) async fn create_file(&mut self, path: &str) -> ExFatResult<FileDetails, D, SIZE> {
        let (dir_path, file_or_dir_name) = split_path(path);
        let mut touched = FileDirty::new();

        // find directory or recursively create it if it does not already exist
        let directory_cluster_id = self.get_or_create_directory(&mut touched, dir_path).await?;
        let flags = GeneralSecondaryFlags::AllocationPossible | GeneralSecondaryFlags::NoFatChain;

        let attributes = FileAttributes::Archive;
        let first_cluster = NO_CLUSTER_ID; // sentinel value for no allocation
        let file_details = self
            .create_file_dir_entry_at(
                &mut touched,
                file_or_dir_name,
                directory_cluster_id,
                first_cluster,
                attributes,
                flags,
                0,
                0,
            )
            .await?;

        touched.flush(self).await?;

        Ok(file_details)
    }

    #[bisync]
    pub(crate) async fn get_or_create_directory(
        &mut self,
        touched: &mut impl Touched,
        path: &str,
    ) -> ExFatResult<u32, D, SIZE> {
        let mut names = path_to_iter(path).peekable();
        let mut cluster_id = self.fs.first_cluster_of_root_dir;

        while let Some(dir_name) = names.next() {
            let is_last = names.peek().is_none();

            let filter = ExactNameFilter::new(
                dir_name,
                &self.upcase_table,
                Some(FileAttributes::Directory),
            );
            let mut entries = DirectoryEntryChain::new(cluster_id, &self.fs);
            let file_details = entries.next_file_entry(self, &filter).await?;

            match file_details {
                Some(file_details) => {
                    // directory already exists
                    cluster_id = file_details.first_cluster;
                }
                None => {
                    // directory does not exist, create it
                    let run = self.allocator.find_free_clusters(&mut self.dev, 1).await?;
                    self.allocator
                        .mark_allocated(&mut self.dev, touched, &run, true)
                        .await?;

                    self.create_file_dir_entry_at(
                        touched,
                        dir_name,
                        cluster_id,
                        run.first_cluster,
                        FileAttributes::Directory,
                        GeneralSecondaryFlags::AllocationPossible
                            | GeneralSecondaryFlags::NoFatChain,
                        self.fs.cluster_length as u64,
                        self.fs.cluster_length as u64,
                    )
                    .await?;

                    cluster_id = run.first_cluster;
                }
            }

            if is_last {
                return Ok(cluster_id);
            }
        }

        // return the root directory cluster
        Ok(cluster_id)
    }

    // assume that the file dir entry does NOT already exist
    // TODO: this is a fairly dangerous assumption, try to enforce it without redundant checks
    // assume that name is valid
    #[allow(clippy::too_many_arguments)]
    #[bisync]
    pub(crate) async fn create_file_dir_entry_at(
        &mut self,
        touched: &mut impl Touched,
        name: &str,
        directory_cluster_id: u32,
        first_cluster: u32, // the directory or file that this entry points to
        file_attributes: FileAttributes,
        stream_ext_flags: GeneralSecondaryFlags,
        valid_data_length: u64,
        data_length: u64,
    ) -> ExFatResult<FileDetails, D, SIZE> {
        let (name_hash, file_name_char_count) =
            encode_utf16_upcase_and_hash(name, &self.upcase_table);
        let dir_entry_set_len = calc_dir_entry_set_len(file_name_char_count);
        let location = self
            .find_empty_dir_entry_set(directory_cluster_id, dir_entry_set_len)
            .await?;
        let location_copy = location;
        let secondary_count = dir_entry_set_len as u8 - 1;

        let mut dir_set_writer = DirSetWriter::new(location);

        // write file directory entry set
        let file = FileDirEntry {
            secondary_count,
            set_checksum: 0,
            file_attributes,
            create_timestamp: 0,
            last_modified_timestamp: 0,
            last_accessed_timestamp: 0,
            create_10ms_increment: 0,
            last_modified_10ms_increment: 0,
            create_utc_offset: 0,
            last_modified_utc_offset: 0,
            last_accessed_utc_offset: 0,
        };
        let file_dir = file.serialize();
        dir_set_writer.add(self, touched, &file_dir, true).await?;

        // write stream extension directory entry
        let stream_ext = StreamExtensionDirEntry {
            general_secondary_flags: stream_ext_flags,
            name_length: file_name_char_count as u8,
            name_hash,
            valid_data_length,
            first_cluster,
            data_length,
        };
        dir_set_writer
            .add(self, touched, &stream_ext.serialize(), false)
            .await?;

        // write file name directory entries chunked by 15 characters
        let mut chunk_iter = Utf16Chunks::new(name);
        loop {
            let mut file_name = FileNameDirEntry {
                general_secondary_flags: GeneralSecondaryFlags::empty(),
                file_name: [0u16; 15],
            };
            if let Some(_len) = chunk_iter.next_chunk(&mut file_name.file_name) {
                dir_set_writer
                    .add(self, touched, &file_name.serialize(), false)
                    .await?;
            } else {
                break;
            }
        }

        dir_set_writer.finish(self, touched, &file_dir).await?;

        let file_details = FileDetails {
            attributes: file_attributes,
            data_length,
            valid_data_length,
            first_cluster,
            flags: stream_ext_flags,
            location: location_copy,
            secondary_count,
        };

        Ok(file_details)
    }

    pub fn unmount(self) -> D {
        self.dev
    }

    #[inline(always)]
    fn mark_dir_entry_free(dir_entry: &mut [u8; RAW_ENTRY_LEN]) {
        dir_entry[0] = 0x01;
    }

    #[bisync]
    async fn get_cluster_id_at(
        &mut self,
        cursor: u64,
        file_details: &FileDetails,
    ) -> ExFatResult<u32, D, SIZE> {
        if cursor == 0 {
            return Ok(file_details.first_cluster);
        }

        let num_clusters = (cursor / self.fs.cluster_length as u64) as u32;

        if file_details
            .flags
            .contains(GeneralSecondaryFlags::NoFatChain)
        {
            let cluster_id = file_details.first_cluster + num_clusters;
            Ok(cluster_id)
        } else {
            let mut cluster_id = file_details.first_cluster;
            for _i in 0..num_clusters - 1 {
                if let Some(x) = self
                    .fat
                    .next_cluster_in_fat_chain(cluster_id, &mut self.dev)
                    .await?
                {
                    cluster_id = x;
                } else {
                    return Err(ExFatError::EndOfFatChain);
                }
            }
            Ok(cluster_id)
        }
    }

    #[bisync]
    async fn confirm_has_no_children(
        &mut self,
        file_details: &FileDetails,
    ) -> ExFatResult<(), D, SIZE> {
        if file_details.attributes.contains(FileAttributes::Directory) {
            let mut reader = DirectoryEntryChain::new(file_details.first_cluster, &self.fs);

            while let Some((dir_entry, _location)) = reader.next(self).await? {
                let entry_type_val = dir_entry[0];
                match EntryType::from(entry_type_val) {
                    EntryType::UnusedOrEndOfDirectory => {
                        // TODO: consider checking for end of directory
                        // as this cluster may contain junk from previous use after the end of directory marker
                        continue;
                    }
                    _ => return Err(ExFatError::DirectoryNotEmpty),
                }
            }
        }

        Ok(())
    }

    // the only difference between deleting a file and a directory is that we must
    // first check tha the directory is empty.
    #[bisync]
    async fn delete_inner(&mut self, file_details: &FileDetails) -> ExFatResult<(), D, SIZE> {
        self.confirm_has_no_children(file_details).await?;
        let mut touched = FileDirty::new();

        // TODO: if this is no-fat-chain then don't use the FAT
        let chain = self.get_stored_chain(file_details).await?;
        self.allocator
            .free(&mut self.dev, &mut touched, &mut self.fat, &chain)
            .await?;

        let mut sector_id = file_details.location.sector_id;
        let dir_entry_offset = file_details.location.dir_entry_offset;

        let mut count = None;
        let mut from = dir_entry_offset;
        loop {
            let slot = self.data_blocks.read_mut(sector_id, &mut self.dev).await?;
            let (dir_entries, _remainder) = slot.as_mut_slice().as_chunks_mut::<RAW_ENTRY_LEN>();

            let count = count.get_or_insert_with(|| {
                let file_dir_entry = FileDirEntry::from(&dir_entries[dir_entry_offset]);
                file_dir_entry.secondary_count as usize
            });

            let to = (from + *count).min(dir_entries.len());
            for dir_entry in &mut dir_entries[from..to] {
                Self::mark_dir_entry_free(dir_entry);
                *count -= 1;
            }

            if *count == 0 {
                break;
            } else {
                sector_id += 1;
                from = 0;
            }
        }

        touched.flush(self).await?;

        Ok(())
    }

    #[bisync]
    async fn find_empty_dir_entry_set(
        &mut self,
        cluster_id: u32,
        dir_entry_set_len: usize,
    ) -> ExFatResult<Location, D, SIZE> {
        let mut entries = DirectoryEntryChain::new(cluster_id, &self.fs);
        let mut counter = 0;
        let mut start = None;
        while let Some((entry, location)) = entries.next(self).await? {
            let entry_type_val = entry[0];
            match EntryType::from(entry_type_val) {
                EntryType::UnusedOrEndOfDirectory => {
                    if start.is_none() {
                        start = Some(location)
                    }
                    counter += 1;

                    if counter == dir_entry_set_len {
                        break;
                    }
                }

                _entry_type => {
                    // slot taken, reset search run
                    counter = 0;
                    start = None
                }
            }
        }

        match start {
            Some(location) => Ok(location),
            None => unimplemented!("growing a directory not yet supported"),
        }
    }
}

fn path_to_iter(path: &str) -> impl Iterator<Item = &str> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(|c| c.trim())
}

#[bisync]
async fn read_boot_sector<D, const SIZE: usize>(
    io: &mut D,
    sector_id: u32,
) -> ExFatResult<BootSector, D, SIZE>
where
    D: BlockDevice<SIZE>,
{
    let mut data = [Aligned([0u8; SIZE])];
    io.read(sector_id, &mut data)
        .await
        .map_err(ExFatError::Io)?;
    let boot_sector: BootSector = (&data[0]).try_into()?;
    // TODO: run checks
    Ok(boot_sector)
}

#[bisync]
pub(crate) async fn read_file_system_metadata<D, const SIZE: usize>(
    io: &mut D,
) -> ExFatResult<FileSystemMetadata<SIZE>, D, SIZE>
where
    D: BlockDevice<SIZE>,
{
    // the boot sector is always at sector_id 0 and everything is relative from there
    // you need to offset the sector_id in your block device if there is a master boot record before this
    let boot_sector = read_boot_sector(io, 0).await?;

    let details = FileSystemDetails::new(&boot_sector);

    let cluster_id = details.first_cluster_of_root_dir;
    let sector_id = details.get_heap_sector_id::<D, SIZE>(cluster_id)?;
    let mut data = [Aligned([0u8; SIZE])];
    io.read(sector_id, &mut data)
        .await
        .map_err(ExFatError::Io)?;

    let mut allocation_bitmap_dir_entry: Option<AllocationBitmapDirEntry> = None;
    let mut volume_label: Option<VolumeLabelDirEntry> = None;
    let mut upcase_table_dir_entry: Option<UpcaseTableDirEntry> = None;

    let (chunks, _remainder) = data[0].as_chunks::<RAW_ENTRY_LEN>();

    for chunk in chunks {
        let entry_type_val = chunk[0];
        match EntryType::from(entry_type_val) {
            EntryType::AllocationBitmap => {
                let entry: AllocationBitmapDirEntry = chunk.into();
                allocation_bitmap_dir_entry = Some(entry);
            }
            EntryType::VolumeLabel => {
                let entry: VolumeLabelDirEntry = chunk.try_into()?;
                volume_label = Some(entry);
            }
            EntryType::UpcaseTable => {
                let entry: UpcaseTableDirEntry = chunk.into();
                upcase_table_dir_entry = Some(entry);
            }
            EntryType::UnusedOrEndOfDirectory if is_end_of_directory(chunk) => {
                break;
            }
            _ => {} // ignore
        }

        if allocation_bitmap_dir_entry.is_some()
            && upcase_table_dir_entry.is_some()
            && volume_label.is_some()
        {
            break;
        }
    }

    let upcase_table_dir_entry = match upcase_table_dir_entry {
        Some(entry) => entry,
        None => {
            return Err(ExFatError::InvalidFileSystem {
                reason: "no upcase table found in root dir",
            });
        }
    };

    if volume_label.is_none() {
        return Err(ExFatError::InvalidFileSystem {
            reason: "no volume label found in root dir",
        });
    }

    let allocation_bitmap_dir_entry = match allocation_bitmap_dir_entry {
        Some(entry) => entry,
        None => {
            return Err(ExFatError::InvalidFileSystem {
                reason: "no allocation bitmap found in root dir",
            });
        }
    };

    let mut upcase_table = UpcaseTable::default();
    upcase_table
        .load(&upcase_table_dir_entry, &details, io)
        .await?;

    let alloc_bitmap = AllocationBitmap::new(&allocation_bitmap_dir_entry);
    Ok(FileSystemMetadata {
        details,
        upcase_table,
        alloc_bitmap,
    })
}

#[allow(unused)]
#[cfg(test)]
mod tests {
    use crate::blocking::file_system;

    use super::super::only_sync;
    use super::*;
    use aligned::Aligned;
    use alloc::{vec, vec::Vec};

    const SECTOR_OFFSET: usize = 0;
    const BLOCK_SIZE: usize = 512;

    #[derive(Debug)]
    struct DummyBlockDevice {
        blocks: Vec<[u8; BLOCK_SIZE]>,
    }

    #[only_sync]
    impl BlockDevice<BLOCK_SIZE> for DummyBlockDevice {
        type Error = ();
        type Align = aligned::A4;

        fn read(
            &mut self,
            block_address: u32,
            data: &mut [Aligned<Self::Align, [u8; BLOCK_SIZE]>],
        ) -> Result<(), Self::Error> {
            data[0].copy_from_slice(&self.blocks[block_address as usize - SECTOR_OFFSET]);
            Ok(())
        }

        fn write(
            &mut self,
            block_address: u32,
            data: &[Aligned<Self::Align, [u8; BLOCK_SIZE]>],
        ) -> Result<(), Self::Error> {
            self.blocks[block_address as usize - SECTOR_OFFSET]
                .copy_from_slice(&data[0].as_slice());
            Ok(())
        }

        fn size(&mut self) -> Result<u64, Self::Error> {
            todo!()
        }
    }

    #[only_sync]
    #[test]
    fn open_file_creates_empty_file() {
        let mut io = DummyBlockDevice {
            blocks: vec![
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
                [0; BLOCK_SIZE],
            ],
        };
        let mut fs = FileSystem::<_, _, 4>::new(io);
        fs.is_mounted = true;
        fs.fs.first_cluster_of_root_dir = 2;
        fs.upcase_table = UpcaseTable::default();
        let options = OpenOptions::new().create(true).write(true);

        let file = fs.open("hello.txt", options).unwrap();

        let dir_entries = &fs.dev.blocks[0][..32 * 3];
        #[rustfmt::skip]
        assert_eq!(
            dir_entries,
            &[
                133, 2, 90, 123, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 
                192, 3, 0, 9, 70, 48, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 
                193, 0, 104, 0, 101, 0, 108, 0, 108, 0, 111, 0, 46, 0, 116, 0, 120, 0, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
    }

    #[only_sync]
    #[test]
    fn write_text_to_file() {
        // arrange
        let mut io = DummyBlockDevice {
            blocks: vec![[0; BLOCK_SIZE]; 128], // 2 clusters
        };
        io.blocks[1][0] = 1; // mark the first cluster (cluster 2) as allocated
        let mut fs = FileSystem::<_, _, 4>::new(io);
        fs.is_mounted = true;
        fs.fs.first_cluster_of_root_dir = 2;
        fs.upcase_table = UpcaseTable::default();
        fs.allocator.bitmap.first_sector = 1;
        fs.allocator.bitmap.num_sectors = 1;
        let options = OpenOptions::new().create(true).write(true);

        // act
        let mut file = fs.open("hello.txt", options).unwrap();
        file.write(&mut fs, b"world").unwrap();
        file.close(&mut fs).unwrap();

        // check first 8 bits of the alloc bitmap
        // it was 0b0000_0001 before the file was allocated
        let alloc_bitmap = fs.dev.blocks[1][0];
        assert_eq!(alloc_bitmap, 0b0000_0011);

        // check the first 3 directory entried of the root directory
        let dir_entries = &fs.dev.blocks[0][..32 * 3];
        #[rustfmt::skip]
        assert_eq!(
            dir_entries,
            &[
                133, 2, 186, 143, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                192, 3, 0, 9, 70, 48, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0,
                193, 0, 104, 0, 101, 0, 108, 0, 108, 0, 111, 0, 46, 0, 116, 0, 120, 0, 116, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );

        // check the first sector of cluster 3
        // there are 64 sectors in a cluster here and clusters start at 2
        let data = &fs.dev.blocks[64][..5];
        assert_eq!(data, b"world");
    }
}

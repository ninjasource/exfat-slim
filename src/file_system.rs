use alloc::{boxed::Box, string::String, vec, vec::Vec};

use super::file::OpenOptions;
use super::{
    allocation_bitmap::{Allocation, AllocationBitmap},
    bisync,
    boot_sector::{BootSector, VolumeFlags},
    directory_entry::{
        AllocationBitmapDirEntry, DirectoryEntryChain, EntryType, FileAttributes, FileDirEntry,
        FileNameDirEntry, GeneralSecondaryFlags, Location, RAW_ENTRY_LEN, RawDirEntry,
        StreamExtensionDirEntry, UpcaseTableDirEntry, VolumeLabelDirEntry, calc_checksum_old,
        is_end_of_directory,
    },
    error::ExFatError,
    fat::next_cluster_in_fat_chain,
    file::{
        DirectoryIterator, ExactNameFilter, File, FileDetails, directory_list, find_file_inner,
        find_file_or_directory, next_file_entry,
    },
    io::{BLOCK_SIZE, Block, BlockDevice},
    only_async,
    upcase_table::UpcaseTable,
    utils::{
        calc_dir_entry_set_len, encode_utf16_and_hash, encode_utf16_upcase_and_hash, read_u16_le,
    },
};

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, Clone)]
pub struct FileSystemDetails {
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
    pub fn new(boot_sector: &BootSector) -> Self {
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

    pub fn get_heap_sector_id(&self, cluster_id: u32) -> Result<u32, ExFatError> {
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
pub struct FileSystem {
    pub fs: FileSystemDetails,
    upcase_table: UpcaseTable,
    pub alloc_bitmap: AllocationBitmap,
}

impl FileSystem {
    pub fn new_inner(
        details: FileSystemDetails,
        upcase_table: UpcaseTable,
        alloc_bitmap: AllocationBitmap,
    ) -> Self {
        Self {
            fs: details,
            upcase_table,
            alloc_bitmap,
        }
    }

    #[bisync]
    pub async fn new(io: &mut impl BlockDevice) -> Result<Self, ExFatError> {
        // the boot sector is always at sector_id 0 and everything is relative from there
        // you need to offset the sector_id in your block device if there is a master boot record before this
        let boot_sector = read_boot_sector(io, 0).await?;
        let details = FileSystemDetails::new(&boot_sector);
        let fs = read_root_dir(io, details).await?;
        Ok(fs)
    }

    #[bisync]
    pub async fn open(
        &self,
        io: &mut impl BlockDevice,
        options: &OpenOptions,
        full_path: &str,
    ) -> Result<File, ExFatError> {
        // The various options do the following:
        // create => check if file exists and use that if it does otherwise open file
        // create_new => check if file exists and return error if it does otherwise create file
        // append => set cursor to end of file
        // truncate => set cursor to start of file and data_length and valid_data_length to 0
        // write => set cursor to start of file and enable writes
        // read => set cursor to start of file and enable reads

        let file_details = find_file_or_directory(
            io,
            &self.fs,
            &self.upcase_table,
            full_path,
            Some(FileAttributes::Archive),
        )
        .await?;
        let file = File::new(&self, &file_details, options);
        Ok(file)
    }

    #[bisync]
    pub async fn exists(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<bool, ExFatError> {
        match find_file_or_directory(io, &self.fs, &self.upcase_table, full_path, None).await {
            Ok(_file) => Ok(true),
            Err(ExFatError::FileNotFound) => Ok(false),
            Err(ExFatError::DirectoryNotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    #[bisync]
    pub async fn read(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<Vec<u8>, ExFatError> {
        let options = OpenOptions::new().read(true).build();
        let mut file = self.open(io, &options, full_path).await?;
        file.read_to_end(io).await
    }

    #[bisync]
    pub async fn read_to_string(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<String, ExFatError> {
        let options = OpenOptions::new().read(true).build();
        let mut file = self.open(io, &options, full_path).await?;
        file.read_to_string(io).await
    }

    #[bisync]
    pub async fn read_dir(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<DirectoryIterator, ExFatError> {
        directory_list(io, &self.fs, &self.upcase_table, full_path).await
    }

    /// deletes a file
    /// returns an error if the file does not exist or something else failed
    #[bisync]
    pub async fn remove_file(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<(), ExFatError> {
        let file_details = find_file_inner(
            io,
            &self.fs,
            &self.upcase_table,
            full_path,
            Some(FileAttributes::Archive),
        )
        .await?;
        self.delete_inner(io, &file_details).await?;
        Ok(())
    }

    /// creates a directory recursively (the dir path can be nested)
    #[bisync]
    pub async fn create_directory(
        &self,
        io: &mut impl BlockDevice,
        dir_path: &str,
    ) -> Result<(), ExFatError> {
        // find directory or recursively create it if it does not already exist
        let _cluster_id = get_or_create_directory(
            io,
            &self.fs,
            &self.upcase_table,
            &self.alloc_bitmap,
            dir_path,
        )
        .await?;

        Ok(())
    }

    /// rename a file or folder
    /// the from_path is the old file or directory path (including all sub directories)
    /// the to_path is the new file or directory path (including all sub directories)
    /// if the file or directory changes parent directories then this is conidered to be a file move, otherwide a rename
    #[bisync]
    pub async fn copy(
        &self,
        io: &mut impl BlockDevice,
        from_path: &str,
        to_path: &str,
    ) -> Result<(), ExFatError> {
        if from_path == to_path {
            return Err(ExFatError::InvalidFileName {
                reason: "cannot copy file to the same exact location",
            });
        }
        let mut file = self
            .open(io, OpenOptions::new().read(true), from_path)
            .await?;

        if let Some((dir_path, file_or_dir_name)) = split_path(to_path) {
            let num_clusters = file
                .valid_data_length
                .div_ceil(self.fs.cluster_length as u64) as u32;

            // find free space on the drive, preferring contiguous clusters
            let allocation = self
                .alloc_bitmap
                .find_free_clusters(io, &self.fs, num_clusters, false)
                .await?;

            self.set_volume_dirty(io, true).await?;

            // find directory or recursively create it if it does not already exist
            let directory_cluster_id = get_or_create_directory(
                io,
                &self.fs,
                &self.upcase_table,
                &self.alloc_bitmap,
                dir_path,
            )
            .await?;

            match allocation {
                Allocation::Contiguous {
                    first_cluster,
                    num_clusters,
                } => {
                    let flags = GeneralSecondaryFlags::AllocationPossible
                        | GeneralSecondaryFlags::NoFatChain;

                    create_file_dir_entry_at(
                        io,
                        &self.upcase_table,
                        file_or_dir_name,
                        directory_cluster_id,
                        first_cluster,
                        file.attributes,
                        flags,
                        file.valid_data_length,
                        file.data_length,
                        &self.fs,
                    )
                    .await?;

                    self.alloc_bitmap
                        .mark_allocated_contiguous(io, &self.fs, first_cluster, num_clusters, true)
                        .await?;

                    let mut sector_id = self.fs.get_heap_sector_id(first_cluster)?;
                    let mut buf = [0u8; BLOCK_SIZE];

                    while let Some(len) = file.read(io, &mut buf).await? {
                        io.write_sector(sector_id, &buf).await?;
                        sector_id += 1;
                    }
                }
                Allocation::FatChain { clusters } => {
                    unimplemented!()
                }
            }

            self.set_volume_dirty(io, false).await?;
        }

        Ok(())
    }

    /// rename a file or folder
    /// the from_path is the old file or directory path (including all sub directories)
    /// the to_path is the new file or directory path (including all sub directories)
    /// if the file or directory changes parent directories then this is conidered to be a file move, otherwide a rename
    #[bisync]
    pub async fn rename(
        &self,
        io: &mut impl BlockDevice,
        from_path: &str,
        to_path: &str,
    ) -> Result<(), ExFatError> {
        // in exFAT a directory cannot have a directory and file with the same name in it so no need to filter here
        let file_details =
            find_file_inner(io, &self.fs, &self.upcase_table, from_path, None).await?;

        // mark dir entries as free
        let mut freed_dir_entries = Vec::with_capacity(1 + file_details.secondary_count as usize);
        for _ in 0..freed_dir_entries.capacity() {
            let mut dir_entry = [0u8; RAW_ENTRY_LEN];
            Self::mark_dir_entry_free(&mut dir_entry);
            freed_dir_entries.push(dir_entry);
        }

        if let Some((dir_path, file_or_dir_name)) = split_path(to_path) {
            self.set_volume_dirty(io, true).await?;

            write_dir_entries_to_disk(io, file_details.location, freed_dir_entries).await?;

            // find directory or recursively create it if it does not already exist
            let directory_cluster_id = get_or_create_directory(
                io,
                &self.fs,
                &self.upcase_table,
                &self.alloc_bitmap,
                dir_path,
            )
            .await?;

            create_file_dir_entry_at(
                io,
                &self.upcase_table,
                file_or_dir_name,
                directory_cluster_id,
                file_details.first_cluster,
                file_details.attributes,
                file_details.flags,
                file_details.valid_data_length,
                file_details.data_length,
                &self.fs,
            )
            .await?;

            self.set_volume_dirty(io, false).await?;
        }

        Ok(())
    }

    #[inline(always)]
    fn mark_dir_entry_free(dir_entry: &mut [u8; RAW_ENTRY_LEN]) {
        dir_entry[0] = 0x01;
    }

    /// removes an empty directory
    /// will return an error if the directory does not exist or is not empty
    #[bisync]
    pub async fn remove_dir(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<(), ExFatError> {
        let file_details = find_file_inner(
            io,
            &self.fs,
            &self.upcase_table,
            full_path,
            Some(FileAttributes::Directory),
        )
        .await?;
        self.delete_inner(io, &file_details).await?;
        Ok(())
    }

    /// writes the entire contents to a file.
    /// the file will be overwritten if it already exists.
    /// this function will create any directories in the path that do not already exist.
    /// relative paths are not supported.
    #[bisync]
    pub async fn write(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
        contents: impl AsRef<[u8]>,
    ) -> Result<(), ExFatError> {
        // delete the file if it already exists
        match find_file_inner(
            io,
            &self.fs,
            &self.upcase_table,
            full_path,
            Some(FileAttributes::Archive),
        )
        .await
        {
            Ok(file_details) => self.delete_inner(io, &file_details).await?,
            Err(ExFatError::FileNotFound) => {
                // ignore
            }
            Err(e) => return Err(e),
        }

        self.write_inner(io, full_path, contents).await?;
        Ok(())
    }

    /// assume that the file does not already exist
    #[bisync]
    async fn write_inner(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
        contents: impl AsRef<[u8]>,
    ) -> Result<(), ExFatError> {
        if let Some((dir_path, file_name)) = split_path(full_path) {
            // find directory or recursively create it if it does not already exist
            let directory_cluster_id = get_or_create_directory(
                io,
                &self.fs,
                &self.upcase_table,
                &self.alloc_bitmap,
                dir_path,
            )
            .await?;

            let contents = contents.as_ref();
            let num_clusters =
                (contents.len() as u64).div_ceil(self.fs.cluster_length as u64) as u32;

            // find free space on the drive, preferring contiguous clusters
            let allocation = self
                .alloc_bitmap
                .find_free_clusters(io, &self.fs, num_clusters, false)
                .await?;

            match allocation {
                Allocation::Contiguous {
                    first_cluster,
                    num_clusters,
                } => {
                    self.set_volume_dirty(io, true).await?;

                    self.alloc_bitmap
                        .mark_allocated_contiguous(io, &self.fs, first_cluster, num_clusters, true)
                        .await?;

                    create_file_dir_entry_at(
                        io,
                        &self.upcase_table,
                        file_name,
                        directory_cluster_id,
                        first_cluster,
                        FileAttributes::Archive,
                        GeneralSecondaryFlags::AllocationPossible
                            | GeneralSecondaryFlags::NoFatChain,
                        contents.len() as u64,
                        contents.len() as u64,
                        &self.fs,
                    )
                    .await?;

                    // write all blocks one after the next
                    let mut sector_id = self.fs.get_heap_sector_id(first_cluster)?;
                    let (chunks, remainder) = contents.as_chunks::<BLOCK_SIZE>();

                    // write all block size chunks
                    for block in chunks {
                        io.write_sector(sector_id, block).await?;
                        sector_id += 1;
                    }

                    // fill the last block the remainder data followed by zeros
                    if remainder.len() > 0 {
                        let mut block = [0u8; BLOCK_SIZE];
                        block[..remainder.len()].copy_from_slice(remainder);
                        io.write_sector(sector_id, &block).await?;
                    }

                    // TODO: figure out what to do if we fail half way through
                    self.set_volume_dirty(io, false).await?;
                }
                Allocation::FatChain { clusters } => {
                    unimplemented!("writing to a file using the fat chain is not yet supported")
                }
            }
        }

        Ok(())
    }

    #[bisync]
    async fn set_volume_dirty(
        &self,
        io: &mut impl BlockDevice,
        is_dirty: bool,
    ) -> Result<(), ExFatError> {
        let sector_id = 0; // boot sector
        let mut sector = [0u8; BLOCK_SIZE];
        sector.copy_from_slice(io.read_sector(sector_id).await?);
        let mut volume_flags = VolumeFlags::from_bits_truncate(read_u16_le::<106, _>(&sector));
        volume_flags.set(VolumeFlags::VolumeDirty, is_dirty);
        sector[106..108].copy_from_slice(&volume_flags.bits().to_le_bytes());
        io.write_sector(sector_id, &sector).await?;
        Ok(())
    }

    #[bisync]
    async fn get_all_clusters_from(
        &self,
        io: &mut impl BlockDevice,
        file_details: &FileDetails,
    ) -> Result<Vec<u32>, ExFatError> {
        let mut cluster_id = file_details.first_cluster;
        let num_clusters = file_details.get_num_clusters(self.fs.cluster_length);

        let mut clusters = Vec::with_capacity(num_clusters);
        clusters.push(cluster_id);

        while let Some(x) = next_cluster_in_fat_chain(io, self.fs.fat_offset, cluster_id).await? {
            cluster_id = x;
            clusters.push(cluster_id);
        }

        Ok(clusters)
    }

    /// the only difference between deleting a file and a directory is that we must
    /// first check tha the directory is empty.
    #[bisync]
    async fn confirm_has_no_children(
        &self,
        io: &mut impl BlockDevice,
        file_details: &FileDetails,
    ) -> Result<(), ExFatError> {
        if file_details.attributes.contains(FileAttributes::Directory) {
            let mut reader = DirectoryEntryChain::new(file_details.first_cluster, &self.fs);

            while let Some((dir_entry, _location)) = reader.next(io).await? {
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

    /// the only difference between deleting a file and a directory is that we must
    /// first check tha the directory is empty.
    #[bisync]
    async fn delete_inner(
        &self,
        io: &mut impl BlockDevice,
        file_details: &FileDetails,
    ) -> Result<(), ExFatError> {
        self.confirm_has_no_children(io, file_details).await?;

        self.set_volume_dirty(io, true).await?;

        // TODO: if this is no-fat-chain then dont use the FAT
        let cluster_ids = self.get_all_clusters_from(io, file_details).await?;
        self.alloc_bitmap
            .mark_allocated(io, &self.fs, &cluster_ids, false)
            .await?;

        let mut sector_id = file_details.location.sector_id;
        let dir_entry_offset = file_details.location.dir_entry_offset;

        let mut sector = [0u8; BLOCK_SIZE];
        sector.copy_from_slice(io.read_sector(file_details.location.sector_id).await?);
        let (dir_entries, _remainder) = sector.as_chunks_mut::<RAW_ENTRY_LEN>();

        let file_dir_entry = FileDirEntry::from(&dir_entries[dir_entry_offset]);
        let mut count = file_dir_entry.secondary_count as usize;
        Self::mark_dir_entry_free(&mut dir_entries[dir_entry_offset]);

        // zero out all directory entries in the directory entry set
        // directory entries can spill over to the next sector but not to the next cluster
        let mut from = dir_entry_offset + 1;
        loop {
            let (dir_entries, _remainder) = sector.as_chunks_mut::<RAW_ENTRY_LEN>();
            let to = (from + count).min(dir_entries.len());
            for dir_entry in &mut dir_entries[from..to] {
                Self::mark_dir_entry_free(dir_entry);
                count -= 1;
            }

            // we need to do this to acount multiple mutable borrows due to the loop
            //  tmp.copy_from_slice(sector.as_slice());
            io.write_sector(sector_id, &sector).await?;

            if count == 0 {
                break;
            } else {
                sector_id += 1;
                let block = io.read_sector(sector_id).await?;
                sector.copy_from_slice(block);
                from = 0;
            }
        }

        self.set_volume_dirty(io, false).await?;
        Ok(())
    }
}

fn path_to_iter(full_path: &str) -> impl Iterator<Item = &str> {
    full_path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(|c| c.trim())
}

#[bisync]
async fn get_or_create_directory(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    alloc_bitmap: &AllocationBitmap,
    dir_path: &str,
) -> Result<u32, ExFatError> {
    let mut names = path_to_iter(dir_path).peekable();
    let mut cluster_id = fs.first_cluster_of_root_dir;

    while let Some(dir_name) = names.next() {
        let is_last = names.peek().is_none();

        let filter = ExactNameFilter::new(dir_name, upcase_table, Some(FileAttributes::Directory));
        let mut entries = DirectoryEntryChain::new(cluster_id, fs);
        let file_details = next_file_entry(io, &mut entries, &filter).await?;

        match file_details {
            Some(file_details) => {
                // directory already exists
                cluster_id = file_details.first_cluster;
            }
            None => {
                // directory does not exist, create it
                let allocation = alloc_bitmap.find_free_clusters(io, fs, 1, true).await?;
                let first_cluster = match allocation {
                    Allocation::FatChain { clusters } => clusters[0],
                    Allocation::Contiguous {
                        first_cluster,
                        num_clusters,
                    } => unreachable!(), // because we passed only_fat_chain = true
                };

                create_file_dir_entry_at(
                    io,
                    upcase_table,
                    dir_name,
                    cluster_id,
                    first_cluster,
                    FileAttributes::Directory,
                    GeneralSecondaryFlags::AllocationPossible | GeneralSecondaryFlags::NoFatChain,
                    fs.cluster_length as u64,
                    fs.cluster_length as u64,
                    fs,
                )
                .await?;

                // mark cluster used to store the directory as allocated
                alloc_bitmap
                    .mark_allocated(io, fs, &[first_cluster], true)
                    .await?;

                cluster_id = first_cluster;
            }
        }

        if is_last {
            return Ok(cluster_id);
        }
    }

    // return the root directory cluster
    Ok(cluster_id)
}

/// assume that the file dir entry does NOT already exist
/// TODO: this is a fairly dangerous assumption, try to enforse it without redundant checks
/// assume that name is valid
#[bisync]
async fn create_file_dir_entry_at(
    io: &mut impl BlockDevice,
    upcase_table: &UpcaseTable,
    name: &str,
    directory_cluster_id: u32,
    first_cluster: u32, // the directory or file that this entry points to
    file_attributes: FileAttributes,
    stream_ext_flags: GeneralSecondaryFlags,
    valid_data_length: u64,
    data_length: u64,
    fs: &FileSystemDetails,
) -> Result<(), ExFatError> {
    let (utf16_name, name_hash) = encode_utf16_and_hash(name, upcase_table);
    let dir_entry_set_len = calc_dir_entry_set_len(&utf16_name);
    let location =
        find_empty_dir_entry_set(io, directory_cluster_id, dir_entry_set_len, fs).await?;
    let mut dir_entries: Vec<RawDirEntry> = Vec::with_capacity(dir_entry_set_len);

    // write file directory entry set
    let file = FileDirEntry {
        secondary_count: dir_entry_set_len as u8 - 1,
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
    dir_entries.push(file.serialize());

    // write stream extension directory entry
    let stream_ext = StreamExtensionDirEntry {
        general_secondary_flags: stream_ext_flags,
        name_length: utf16_name.len() as u8,
        name_hash,
        valid_data_length,
        first_cluster,
        data_length,
    };
    dir_entries.push(stream_ext.serialize());

    // write file name directory entries chunked by 15 characters
    let (chunks, remainder) = utf16_name.as_chunks::<15>();
    for chunk in chunks {
        let file_name = FileNameDirEntry {
            general_secondary_flags: GeneralSecondaryFlags::empty(),
            file_name: *chunk,
        };
        dir_entries.push(file_name.serialize());
    }
    if remainder.len() > 0 {
        // any file name ness than 15 characters gets zeros after the name
        let mut file_name = FileNameDirEntry {
            general_secondary_flags: GeneralSecondaryFlags::empty(),
            file_name: [0u16; 15],
        };
        file_name.file_name[..remainder.len()].copy_from_slice(remainder);
        dir_entries.push(file_name.serialize());
    }

    // calculate and update the set_checksum field
    let set_checksum = calc_checksum_old(&dir_entries);
    dir_entries[0][2..4].copy_from_slice(&set_checksum.to_le_bytes());

    // write to disk
    write_dir_entries_to_disk(io, location, dir_entries).await?;
    Ok(())
}

#[bisync]
async fn write_dir_entries_to_disk(
    io: &mut impl BlockDevice,
    location: Location,
    dir_entries: Vec<RawDirEntry>,
) -> Result<(), ExFatError> {
    let mut sector_id = location.sector_id;
    let mut offset = location.dir_entry_offset * RAW_ENTRY_LEN;
    let mut block = Box::new([0u8; BLOCK_SIZE]);
    block.copy_from_slice(io.read_sector(location.sector_id).await?);

    for (index, dir_entry) in dir_entries.iter().enumerate() {
        block[offset..offset + RAW_ENTRY_LEN].copy_from_slice(dir_entry);
        offset += RAW_ENTRY_LEN;

        // move to the next sector if required
        if offset >= BLOCK_SIZE {
            io.write_sector(sector_id, &block).await?;

            // if this is the last entry
            if index == dir_entries.len() - 1 {
                break;
            }

            // we dont need to check if sector_id has overflowed the cluster
            // because we asked for a valid dir entry set
            offset = 0;
            sector_id += 1;
            block.copy_from_slice(io.read_sector(sector_id).await?);
        }
    }

    if offset < BLOCK_SIZE {
        io.write_sector(sector_id, &block).await?;
    }

    Ok(())
}

#[bisync]
async fn find_empty_dir_entry_set(
    io: &mut impl BlockDevice,
    cluster_id: u32,
    dir_entry_set_len: usize,
    fs: &FileSystemDetails,
) -> Result<Location, ExFatError> {
    let mut entries = DirectoryEntryChain::new(cluster_id, fs);
    let mut counter = 0;
    let mut start = None;
    while let Some((entry, location)) = entries.next(io).await? {
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

fn split_path(full_path: &str) -> Option<(&str, &str)> {
    full_path
        .rfind(['/', '\\'])
        .map(|index| (full_path[..index].trim(), full_path[index + 1..].trim()))
}

#[bisync]
async fn read_boot_sector(
    io: &mut impl BlockDevice,
    sector_id: u32,
) -> Result<BootSector, ExFatError> {
    let buf = io.read_sector(sector_id).await?;
    let boot_sector: BootSector = buf.try_into()?;
    // TODO: run checks
    Ok(boot_sector)
}

#[bisync]
async fn read_root_dir(
    io: &mut impl BlockDevice,
    details: FileSystemDetails,
) -> Result<FileSystem, ExFatError> {
    let cluster_id = details.first_cluster_of_root_dir;
    let sector_id = details.get_heap_sector_id(cluster_id)?;
    let buf = io.read_sector(sector_id).await?;

    let mut allocation_bitmap_dir_entry: Option<AllocationBitmapDirEntry> = None;
    let mut volume_label: Option<VolumeLabelDirEntry> = None;
    let mut upcase_table_dir_entry: Option<UpcaseTableDirEntry> = None;

    let (chunks, _remainder) = buf.as_chunks::<RAW_ENTRY_LEN>();

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
            EntryType::UnusedOrEndOfDirectory => {
                if is_end_of_directory(chunk) {
                    break;
                }
            }
            _entry_type => {} // ignore
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

    let file_system = FileSystem::new_inner(details, upcase_table, alloc_bitmap);

    Ok(file_system)
}

#[only_async]
#[cfg(test)]
mod tests {

    use alloc::vec;

    use crate::asynchronous::{
        allocation_bitmap::AllocationBitmap,
        error::ExFatError,
        file_system::{FileSystem, FileSystemDetails},
        io::BLOCK_SIZE,
        mocks::InMemoryBlockDevice,
        upcase_table::UpcaseTable,
    };

    fn dummy_fs() -> FileSystem {
        let details = FileSystemDetails {
            cluster_heap_offset: 2,
            fat_offset: 1,          // fat table will consime 2 sectors
            sectors_per_cluster: 1, // very small cluster size
            cluster_length: 15,
            first_cluster_of_root_dir: 3,
        };

        let upcase_table = UpcaseTable::default();

        let alloc_bitmap = AllocationBitmap {
            first_cluster: 2,
            num_sectors: details.cluster_length.div_ceil(BLOCK_SIZE as u32),
            max_cluster_id: details.cluster_length,
        };

        FileSystem::new_inner(details, upcase_table, alloc_bitmap)
    }

    #[tokio::test]
    async fn create_empty_dir_in_root() -> Result<(), ExFatError> {
        //   env_logger::init();
        let mut sectors = vec![[0; BLOCK_SIZE]; 20];
        sectors[2][0] = 0xF0; // mark the first 4 clusters as used

        let mut io = InMemoryBlockDevice {
            sectors: &mut sectors,
        };

        let fs = dummy_fs();
        let directory = "/hello";

        let exists = fs.exists(&mut io, directory).await?;
        assert!(!exists);

        fs.create_directory(&mut io, directory).await?;

        let exists = fs.exists(&mut io, directory).await?;
        assert!(exists);

        Ok(())
    }
}

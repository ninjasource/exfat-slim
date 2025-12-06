use alloc::{boxed::Box, string::String, vec::Vec};

use crate::{
    allocation_bitmap::{Allocation, AllocationBitmap},
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
        DirectoryIterator, ExactNameFilter, FileDetails, ReadOnlyFile, directory_list, find_file,
        find_file_inner, next_file_entry,
    },
    io::{BLOCK_SIZE, Block, BlockDevice},
    upcase_table::UpcaseTable,
    utils::{calc_dir_entry_set_len, encode_utf16_and_hash, read_u16_le},
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("invalid cluster id ({0})")]
    InvalidClusterId(u32),
}

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

    pub fn get_heap_sector_id(&self, cluster_id: u32) -> Result<u32, Error> {
        if cluster_id < 2 {
            return Err(Error::InvalidClusterId(cluster_id));
        }

        let sector_id =
            self.cluster_heap_offset + (cluster_id - 2) * self.sectors_per_cluster as u32;
        Ok(sector_id)
    }
}

#[derive(Debug)]
pub struct FileSystem {
    details: FileSystemDetails,
    upcase_table: UpcaseTable,
    alloc_bitmap: AllocationBitmap,
}

impl FileSystem {
    pub fn new_inner(
        details: FileSystemDetails,
        upcase_table: UpcaseTable,
        alloc_bitmap: AllocationBitmap,
    ) -> Self {
        Self {
            details,
            upcase_table,
            alloc_bitmap,
        }
    }

    pub async fn new(io: &mut impl BlockDevice) -> Result<Self, ExFatError> {
        // the boot sector is always at sector_id 0 and everything is relative from there
        // you need to offset the sector_id in your block device if there is a master boot record before this
        let boot_sector = read_boot_sector(io, 0).await?;
        let details = FileSystemDetails::new(&boot_sector);
        let fs = read_root_dir(io, details).await?;
        Ok(fs)
    }

    pub async fn open(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<ReadOnlyFile, ExFatError> {
        let file_details = find_file(io, &self.details, &self.upcase_table, full_path).await?;
        let file = ReadOnlyFile::new(&self.details, &file_details);
        Ok(file)
    }

    pub async fn exists(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<bool, ExFatError> {
        match find_file(io, &self.details, &self.upcase_table, full_path).await {
            Ok(_file) => Ok(true),
            Err(ExFatError::FileNotFound) => Ok(false),
            Err(ExFatError::DirectoryNotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub async fn read(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<Vec<u8>, ExFatError> {
        let mut file = self.open(io, full_path).await?;
        file.read_to_end(io).await
    }

    pub async fn read_to_string(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<String, ExFatError> {
        let mut file = self.open(io, full_path).await?;
        file.read_to_string(io).await
    }

    pub async fn read_dir(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
    ) -> Result<DirectoryIterator, ExFatError> {
        directory_list(io, &self.details, &self.upcase_table, full_path).await
    }

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

        // TODO: do we need to update a checkum somewhere as a result of this?

        io.write_sector(sector_id, &sector).await?;
        Ok(())
    }

    async fn get_all_clusters_from(
        &self,
        io: &mut impl BlockDevice,
        file_details: &FileDetails,
    ) -> Result<Vec<u32>, ExFatError> {
        let mut cluster_id = file_details.first_cluster;
        let num_clusters = file_details.get_num_clusters(self.details.cluster_length);

        let mut clusters = Vec::with_capacity(num_clusters);
        clusters.push(cluster_id);

        while let Some(x) =
            next_cluster_in_fat_chain(io, self.details.fat_offset, cluster_id).await?
        {
            cluster_id = x;
            clusters.push(cluster_id);
        }

        Ok(clusters)
    }

    async fn delete(
        &self,
        io: &mut impl BlockDevice,
        file_details: &FileDetails,
    ) -> Result<(), ExFatError> {
        if file_details.attributes.contains(FileAttributes::Archive) {
            let cluster_ids = self.get_all_clusters_from(io, file_details).await?;
            self.alloc_bitmap
                .mark_allocated(io, &self.details, &cluster_ids, false)
                .await?;

            let mut sector_id = file_details.location.sector_id;
            let dir_entry_offset = file_details.location.dir_entry_offset;

            let mut sector = [0u8; BLOCK_SIZE];
            sector.copy_from_slice(io.read_sector(file_details.location.sector_id).await?);
            let (dir_entries, _remainder) = sector.as_chunks_mut::<RAW_ENTRY_LEN>();

            let file_dir_entry = FileDirEntry::from(&dir_entries[dir_entry_offset]);
            let mut count = file_dir_entry.secondary_count as usize;
            dir_entries[dir_entry_offset].fill(0);

            // zero out all directory entries in the directory entry set
            // directory entries can spill over to the next sector but not to the next cluster
            let mut from = dir_entry_offset + 1;
            loop {
                let (dir_entries, _remainder) = sector.as_chunks_mut::<RAW_ENTRY_LEN>();
                let to = (from + count).min(dir_entries.len());
                for dir_entry in &mut dir_entries[from..to] {
                    dir_entry.fill(0);
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

            // TODO: keep rreading past sector, remember to save sector first
        } else if file_details.attributes.contains(FileAttributes::Directory) {
            // TODO: delete directory
        }

        Ok(())
    }

    pub async fn write(
        &self,
        io: &mut impl BlockDevice,
        full_path: &str,
        contents: impl AsRef<[u8]>,
    ) -> Result<(), ExFatError> {
        // delete the file if it already exists
        match find_file_inner(io, &self.details, &self.upcase_table, full_path).await {
            Ok(file_details) => self.delete(io, &file_details).await?,
            Err(ExFatError::FileNotFound) => {
                // ignore
            }
            Err(e) => return Err(e),
        }

        if let Some((dir_path, file_name)) = split_path(full_path) {
            //let (utf16_file_name, hash) = encode_utf16_and_hash(file_name, &self.upcase_table);
            //let dir_entry_set_len = calc_dir_entry_set_len(&utf16_file_name);

            let directory_cluster_id = get_or_create_directory(
                io,
                &self.details,
                &self.upcase_table,
                &self.alloc_bitmap,
                dir_path,
            )
            .await?;

            // let location = find_empty_dir_entry_set(io, directory_cluster_id, dir_entry_set_len).await?;
            let contents = contents.as_ref();
            let num_clusters =
                (contents.len() as u64).div_ceil(self.details.cluster_length as u64) as u32;
            let allocation = self
                .alloc_bitmap
                .find_free_clusters(io, &self.details, num_clusters, false)
                .await?;
            match allocation {
                Allocation::Contiguous { first_cluster } => {
                    self.set_volume_dirty(io, true).await?;

                    self.alloc_bitmap
                        .mark_allocated_contiguous(
                            io,
                            &self.details,
                            first_cluster,
                            num_clusters,
                            true,
                        )
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
                    )
                    .await?;

                    // write all blocks one after the next
                    let mut sector_id = self.details.get_heap_sector_id(first_cluster)?;
                    let (chunks, remainder) = contents.as_chunks::<BLOCK_SIZE>();
                    for block in chunks {
                        io.write_sector(sector_id, block).await?;
                        sector_id += 1;
                    }
                    if remainder.len() > 0 {
                        let mut block = [0u8; BLOCK_SIZE];
                        block[..remainder.len()].copy_from_slice(remainder);
                        io.write_sector(sector_id, &block).await?;
                    }

                    // TODO: figure out what to do if we fail half way through
                    self.set_volume_dirty(io, false).await?;
                }
                Allocation::FatChain { first_cluster } => {}
            }

            // TODO: attempt to find no-fatchain congiguous heap records
            // TODO: write the directory entries, saving filled up sectors
            // TODO calc checksum for file
            // TODO: write the actual contents of the file to clusters
            // let check_for_size =
        }

        // TODO: create file and write the contents
        Ok(())
    }
}

fn path_to_iter(full_path: &str) -> impl Iterator<Item = &str> {
    full_path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(|c| c.trim())
}

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

        let filter = ExactNameFilter::new(dir_name, upcase_table, true);
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
                    Allocation::FatChain { first_cluster } => first_cluster,
                    Allocation::Contiguous { first_cluster } => unreachable!(),
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
                )
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
async fn create_file_dir_entry_at(
    io: &mut impl BlockDevice,
    upcase_table: &UpcaseTable,
    name: &str,
    directory_cluster_id: u32,
    first_cluster: u32, // the directory or file that this entry points to
    file_attributes: FileAttributes,
    stream_ext_flags: GeneralSecondaryFlags,
    data_length: u64,
) -> Result<(), ExFatError> {
    let (utf16_name, name_hash) = encode_utf16_and_hash(name, upcase_table);
    let dir_entry_set_len = calc_dir_entry_set_len(&utf16_name);
    let location = find_empty_dir_entry_set(io, directory_cluster_id, dir_entry_set_len).await?;
    let cluster_id = find_empty_cluster(io).await?;

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
        valid_data_length: data_length,
        first_cluster: cluster_id,
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

async fn find_empty_cluster(io: &mut impl BlockDevice) -> Result<u32, ExFatError> {
    todo!()
}

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

async fn find_empty_dir_entry_set(
    io: &mut impl BlockDevice,
    cluster_id: u32,
    dir_entry_set_len: usize,
) -> Result<Location, ExFatError> {
    todo!()
}

fn split_path(full_path: &str) -> Option<(&str, &str)> {
    full_path
        .rfind(['/', '\\'])
        .map(|index| (full_path[..index].trim(), full_path[index..].trim()))
}

async fn read_boot_sector(
    io: &mut impl BlockDevice,
    sector_id: u32,
) -> Result<BootSector, ExFatError> {
    let buf = io.read_sector(sector_id).await?;
    let boot_sector: BootSector = buf.try_into()?;
    // TODO: run checks
    Ok(boot_sector)
}

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

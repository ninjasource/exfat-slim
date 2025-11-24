use alloc::{string::String, vec::Vec};

use crate::{
    boot_sector::BootSector,
    directory_entry::{
        AllocationBitmapDirEntry, EntryType, RAW_ENTRY_LEN, UpcaseTableDirEntry,
        VolumeLabelDirEntry, is_end_of_directory,
    },
    error::ExFatError,
    file::{DirectoryIterator, ReadOnlyFile, directory_list, find_file},
    io::BlockDevice,
    upcase_table::UpcaseTable,
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
}

impl FileSystem {
    pub fn new_inner(details: FileSystemDetails, upcase_table: UpcaseTable) -> Self {
        Self {
            details,
            upcase_table,
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

    read_allocation_bitmap(io, &details, &allocation_bitmap_dir_entry).await?;
    let mut upcase_table = UpcaseTable::default();
    upcase_table
        .load(&upcase_table_dir_entry, &details, io)
        .await?;

    let file_system = FileSystem::new_inner(details, upcase_table);
    Ok(file_system)
}

async fn read_allocation_bitmap(
    io: &mut impl BlockDevice,
    fs: &FileSystemDetails,
    entry: &AllocationBitmapDirEntry,
) -> Result<(), ExFatError> {
    let sector_id = fs.get_heap_sector_id(entry.first_cluster)?;
    let _buf = io.read_sector(sector_id).await?;
    Ok(())
}

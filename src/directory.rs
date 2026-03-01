use alloc::{string::String, vec::Vec};

use super::{
    bisync,
    directory_entry::{DirectoryEntryChain, FileAttributes},
    error::ExFatError,
    file::{FileDetails, Metadata},
    file_system::FileSystemDetails,
    io::BlockDevice,
    upcase_table::UpcaseTable,
    utils::encode_utf16_upcase_and_hash,
};

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
    pub(crate) fn new(
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
pub(crate) async fn get_leaf_file_entry<D: BlockDevice>(
    io: &D,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    path: &str,
    file_attributes: Option<FileAttributes>,
) -> Result<Option<FileDetails>, ExFatError<D>> {
    let mut splits = path
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

        let filter = ExactNameFilter::new(part, upcase_table, attributes);
        let mut entries = DirectoryEntryChain::new(cluster_id, fs, io.clone());
        let file_details = entries.next_file_entry(&filter).await?;

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
pub(crate) async fn directory_list<D: BlockDevice>(
    io: &D,
    fs: &FileSystemDetails,
    upcase_table: &UpcaseTable,
    path: &str,
) -> Result<DirectoryIterator<D>, ExFatError<D>> {
    let cluster_id = if is_root_directory(path) {
        fs.first_cluster_of_root_dir
    } else {
        match get_leaf_file_entry(io, fs, upcase_table, path, Some(FileAttributes::Directory))
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

    let entries = DirectoryEntryChain::new(cluster_id, fs, io.clone());
    Ok(DirectoryIterator { entries })
}

pub struct DirectoryIterator<D: BlockDevice> {
    entries: DirectoryEntryChain<D>,
}

#[derive(Debug)]
pub struct DirectoryEntry {
    details: FileDetails,
}

impl DirectoryEntry {
    /// file or directly name
    pub fn file_name(&self) -> String {
        self.details.name.clone()
    }

    /// metadata for the file or directory
    pub fn metadata(&self) -> Metadata {
        Metadata {
            details: self.details.clone(),
        }
    }
}

impl<D: BlockDevice> DirectoryIterator<D> {
    #[bisync]
    pub async fn next_entry(&mut self) -> Result<Option<DirectoryEntry>, ExFatError<D>> {
        let filter = AllPassFilter {};
        Ok(self
            .entries
            .next_file_entry(&filter)
            .await?
            .map(|x| DirectoryEntry { details: x.clone() }))
    }
}

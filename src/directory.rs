use alloc::{string::String, vec::Vec};

use super::{
    BlockDevice, bisync,
    directory_entry::{DirectoryEntryChain, FileAttributes},
    error::ExFatError,
    file::{FileDetails, Metadata},
    file_system::FileSystem,
    upcase_table::UpcaseTable,
    utils::encode_utf16_upcase_and_hash,
};

pub(crate) trait DirectoryEntryFilter {
    fn hash(&self, file_name_hash: u16, file_attributes: FileAttributes) -> bool;
    fn file_name(&self, file_name: &[u16], upcase_table: &UpcaseTable) -> bool;
}

pub(crate) struct AllPassFilter {}

impl DirectoryEntryFilter for AllPassFilter {
    fn hash(&self, _file_name_hash: u16, _file_attributes: FileAttributes) -> bool {
        true
    }

    fn file_name(&self, _file_name: &[u16], _upcase_table: &UpcaseTable) -> bool {
        true
    }
}

pub(crate) struct ExactNameFilter {
    file_name: Vec<u16>,
    file_name_hash: u16,
    file_attributes: Option<FileAttributes>,
}

impl ExactNameFilter {
    pub(crate) fn new(
        file_name_str: &str,
        upcase_table: &UpcaseTable,
        file_attributes: Option<FileAttributes>,
    ) -> Self {
        let (file_name, file_name_hash) = encode_utf16_upcase_and_hash(file_name_str, upcase_table);
        Self {
            file_name,
            file_name_hash,
            file_attributes,
        }
    }
}

impl DirectoryEntryFilter for ExactNameFilter {
    fn hash(&self, file_name_hash: u16, file_attributes: FileAttributes) -> bool {
        match self.file_attributes {
            Some(attributes) => {
                self.file_name_hash == file_name_hash && file_attributes.contains(attributes)
            }
            None => self.file_name_hash == file_name_hash,
        }
    }

    fn file_name(&self, file_name: &[u16], upcase_table: &UpcaseTable) -> bool {
        // perform case insensitive name match
        for (left, right) in self.file_name.iter().zip(file_name.iter()) {
            let upcased = upcase_table.upcase(*right);
            if *left != upcased {
                // name does not match
                return false;
            }
        }

        true
    }
}

#[bisync]
pub(crate) async fn get_leaf_file_entry<D, const SIZE: usize, const N: usize>(
    fs: &mut FileSystem<D, SIZE, N>,
    path: &str,
    file_attributes: Option<FileAttributes>,
) -> Result<Option<FileDetails>, ExFatError<D, SIZE>>
where
    D: BlockDevice<SIZE>,
{
    let mut splits = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(|c| c.trim())
        .peekable();

    let mut cluster_id = fs.fs.first_cluster_of_root_dir;

    while let Some(part) = splits.next() {
        let is_last = splits.peek().is_none();
        let attributes = if is_last {
            file_attributes
        } else {
            Some(FileAttributes::Directory)
        };

        let filter = ExactNameFilter::new(part, &fs.upcase_table, attributes);
        let mut entries = DirectoryEntryChain::new(cluster_id, &fs.fs);
        let file_details = entries.next_file_entry(fs, &filter).await?;

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
pub(crate) async fn directory_list<D, const SIZE: usize, const N: usize>(
    fs: &mut FileSystem<D, SIZE, N>,
    path: &str,
) -> Result<DirectoryIterator<SIZE>, ExFatError<D, SIZE>>
where
    D: BlockDevice<SIZE>,
{
    let cluster_id = if is_root_directory(path) {
        fs.fs.first_cluster_of_root_dir
    } else {
        match get_leaf_file_entry(fs, path, Some(FileAttributes::Directory)).await? {
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

    let entries = DirectoryEntryChain::new(cluster_id, &fs.fs);
    Ok(DirectoryIterator { entries })
}

pub struct DirectoryIterator<const SIZE: usize> {
    entries: DirectoryEntryChain<SIZE>,
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

impl<const SIZE: usize> DirectoryIterator<SIZE> {
    #[bisync]
    pub async fn next_entry<D, const N: usize>(
        &mut self,
        fs: &mut FileSystem<D, SIZE, N>,
    ) -> Result<Option<DirectoryEntry>, ExFatError<D, SIZE>>
    where
        D: BlockDevice<SIZE>,
    {
        let filter = AllPassFilter {};
        Ok(self
            .entries
            .next_file_entry(fs, &filter)
            .await?
            .map(|x| DirectoryEntry { details: x.clone() }))
    }
}

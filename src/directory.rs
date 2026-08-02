use core::str::from_utf8;

#[cfg(feature = "alloc")]
use alloc::string::String;

use super::{
    BlockDevice, bisync,
    directory_entry::{DirectoryEntryChain, FileAttributes},
    error::ExFatError,
    file::{FileDetails, Metadata},
    file_system::{ExFatResult, FileSystem},
    upcase_table::UpcaseTable,
    utils::encode_utf16_upcase_and_hash,
};

/// The maximum number of bytes required to store the longest possible file name in utf8
pub const MAX_NAME_LEN: usize = 765;

pub(crate) trait DirectoryEntryFilter {
    fn hash(&self, file_name_hash: u16, file_attributes: FileAttributes) -> bool;
    fn file_name_length(&self, length: usize) -> bool;
    fn file_name(&self, file_name: &[u16], ordinal: usize, upcase_table: &UpcaseTable) -> bool;
}

pub(crate) struct AllPassFilter {}

impl DirectoryEntryFilter for AllPassFilter {
    fn hash(&self, _file_name_hash: u16, _file_attributes: FileAttributes) -> bool {
        true
    }

    fn file_name(&self, _file_name: &[u16], _ordinal: usize, _upcase_table: &UpcaseTable) -> bool {
        true
    }

    fn file_name_length(&self, _length: usize) -> bool {
        true
    }
}

pub(crate) struct ExactNameFilter<'a> {
    file_name: &'a str,
    file_name_hash: u16,
    file_attributes: Option<FileAttributes>,
    file_name_count: usize,
}

impl<'a> ExactNameFilter<'a> {
    pub(crate) fn new(
        file_name: &'a str,
        upcase_table: &UpcaseTable,
        file_attributes: Option<FileAttributes>,
    ) -> Self {
        let (file_name_hash, file_name_count) =
            encode_utf16_upcase_and_hash(file_name, upcase_table);
        Self {
            file_name,
            file_name_hash,
            file_attributes,
            file_name_count,
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

    fn file_name(
        &self,
        file_name_part: &[u16],
        ordinal: usize,
        upcase_table: &UpcaseTable,
    ) -> bool {
        // perform case insensitive name match
        for (left, right) in self
            .file_name
            .encode_utf16()
            .skip(ordinal)
            .zip(file_name_part.iter())
        {
            let upcased_left = upcase_table.upcase(left);
            let upcased_right = upcase_table.upcase(*right);
            if upcased_left != upcased_right {
                // name does not match
                return false;
            }
        }

        true
    }

    // the number of utf-16 units (an emoji counts as 2 u16s)
    fn file_name_length(&self, length: usize) -> bool {
        self.file_name_count == length
    }
}

#[bisync]
pub(crate) async fn get_leaf_file_entry<D, const SIZE: usize, const N: usize>(
    fs: &mut FileSystem<D, SIZE, N>,
    path: &str,
    file_attributes: Option<FileAttributes>,
) -> ExFatResult<Option<FileDetails>, D, SIZE>
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
        let file_details = entries.next_file_entry(fs, &filter, None).await?;

        match file_details {
            Some((file_details, _name_buf_len)) => {
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
) -> ExFatResult<DirectoryIterator<SIZE>, D, SIZE>
where
    D: BlockDevice<SIZE>,
{
    let cluster_id = if is_root_directory(path) {
        fs.fs.first_cluster_of_root_dir
    } else {
        match get_leaf_file_entry(fs, path, Some(FileAttributes::Directory)).await? {
            Some(file_details) if file_details.attributes.contains(FileAttributes::Directory) => {
                file_details.first_cluster
            }
            _ => return Err(ExFatError::DirectoryNotFound),
        }
    };

    let entries = DirectoryEntryChain::new(cluster_id, &fs.fs);
    Ok(DirectoryIterator { entries })
}

pub struct DirectoryIterator<const SIZE: usize> {
    entries: DirectoryEntryChain<SIZE>,
}

#[derive(Debug)]
#[cfg(feature = "alloc")]
#[non_exhaustive]
pub struct DirectoryEntryOwned {
    pub metadata: Metadata,
    pub name: String,
}

#[derive(Debug)]
#[non_exhaustive]
pub struct DirectoryEntry<'a> {
    pub metadata: Metadata,
    pub name: &'a str,
}

impl<const SIZE: usize> DirectoryIterator<SIZE> {
    // if name_buf is too small to hold a file or directory name (just the name, not the whole path)
    // then a FileNameBufferTooSmall error is retured
    #[bisync]
    pub async fn next_entry<'a, D, const N: usize>(
        &mut self,
        fs: &mut FileSystem<D, SIZE, N>,
        name_buf: &'a mut [u8], // should be large enough to hold a file name of 255 unicode chars (MAX_NAME_LEN bytes)
    ) -> ExFatResult<Option<DirectoryEntry<'a>>, D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        let filter = AllPassFilter {};
        let entry = self
            .entries
            .next_file_entry(fs, &filter, Some(name_buf))
            .await?;

        match entry {
            Some((details, utf8_name_len)) => {
                let metadata = Metadata { details };
                let name =
                    from_utf8(&name_buf[..utf8_name_len]).map_err(|_| ExFatError::Utf8Error)?;
                let dir_file_entry = DirectoryEntry { metadata, name };
                Ok(Some(dir_file_entry))
            }
            None => Ok(None),
        }
    }
}

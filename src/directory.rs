use core::char::decode_utf16;

#[cfg(feature = "alloc")]
use alloc::string::String;

use super::{
    BlockDevice, bisync,
    directory_entry::{
        DirectoryEntryChain, FileAttributes, FileNameDirEntry, StreamExtensionDirEntry,
    },
    error::ExFatError,
    file::{FileDetails, Metadata},
    file_system::{ExFatResult, FileSystem},
    upcase_table::UpcaseTable,
    utils::encode_utf16_upcase_and_hash,
};

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
}

impl<'a> ExactNameFilter<'a> {
    pub(crate) fn new(
        file_name: &'a str,
        upcase_table: &UpcaseTable,
        file_attributes: Option<FileAttributes>,
    ) -> Self {
        let (file_name_hash, _file_name_count) =
            encode_utf16_upcase_and_hash(file_name, upcase_table);
        Self {
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

    fn file_name_length(&self, length: usize) -> bool {
        self.file_name.len() == length
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
pub struct DirectoryEntry {
    details: FileDetails,
}

impl DirectoryEntry {
    /// file or directly name using a buffer
    #[bisync]
    pub async fn file_name_into<'a, D, const SIZE: usize, const N: usize>(
        &self,
        fs: &mut FileSystem<D, SIZE, N>,
        buf: &'a mut [u8],
    ) -> ExFatResult<&'a str, D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        let mut chain = DirectoryEntryChain::new_from_file_details(&self.details, &fs.fs);
        let mut count = 0;
        let mut name_len = 0;
        let mut cursor = 0;

        while let Some((dir_entry, _location)) = chain.next(fs).await? {
            match count {
                0 => {
                    // ignore
                }
                1 => {
                    let stream_ext: StreamExtensionDirEntry = dir_entry.into();
                    name_len = stream_ext.name_length as usize;
                }
                _ => {
                    if name_len == 0 {
                        return Ok("");
                    }

                    let file_name: FileNameDirEntry = dir_entry.into();
                    let len = file_name.file_name.len().min(name_len - cursor);
                    for ch in decode_utf16((file_name.file_name[..len]).iter().copied()) {
                        let ch = ch.map_err(|_| ExFatError::InvalidUtf16String {
                            reason: "file name contains an invalif utf16 character",
                        })?;

                        let needed = ch.len_utf8();
                        if cursor + needed < buf.len() {
                            ch.encode_utf8(&mut buf[cursor..cursor + needed]);
                            cursor += needed;
                        } else {
                            return Err(ExFatError::FileNameBufferTooSmall);
                        }
                    }
                }
            }

            count += 1;
        }

        let s = core::str::from_utf8(&buf[..cursor]).map_err(|_| ExFatError::Utf8Error)?;
        Ok(s)
    }

    /// file or directly name
    #[cfg(feature = "alloc")]
    #[bisync]
    pub async fn file_name<'a, D, const SIZE: usize, const N: usize>(
        &self,
        fs: &mut FileSystem<D, SIZE, N>,
    ) -> ExFatResult<String, D, SIZE>
    where
        D: BlockDevice<SIZE>,
    {
        let mut chain = DirectoryEntryChain::new_from_file_details(&self.details, &fs.fs);
        let mut count = 0;
        let mut name_len = 0;
        let mut cursor = 0;
        let mut s = String::new();

        while let Some((dir_entry, _location)) = chain.next(fs).await? {
            match count {
                0 => {
                    // ignore
                }
                1 => {
                    let stream_ext: StreamExtensionDirEntry = dir_entry.into();
                    name_len = stream_ext.name_length as usize;

                    // best guess on how much space this will take up (may take more for unicode chars)
                    s = String::with_capacity(name_len);
                }
                _ => {
                    if name_len == 0 {
                        return Ok(String::new());
                    }

                    let file_name: FileNameDirEntry = dir_entry.into();
                    let len = file_name.file_name.len().min(name_len - cursor);
                    for ch in decode_utf16((&file_name.file_name[..len]).iter().copied()) {
                        let ch = ch.map_err(|_| ExFatError::InvalidUtf16String {
                            reason: "file name contains an invalif utf16 character",
                        })?;

                        s.push(ch);
                        cursor += 1;
                    }
                }
            }

            count += 1;
        }
        Ok(s)
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
    ) -> ExFatResult<Option<DirectoryEntry>, D, SIZE>
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

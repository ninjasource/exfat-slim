use thiserror::Error;

use crate::{boot_sector, directory_entry, file_system, io};

#[derive(Error, Debug)]
pub enum ExFatError {
    #[error("io")]
    Io(#[from] io::Error),

    #[error("directory entry ({0:?})")]
    DirectoryEntry(#[from] directory_entry::Error),

    #[error("boot sector ({0:?})")]
    BootSector(#[from] boot_sector::Error),

    #[error("file system ({0:?})")]
    FileSystem(#[from] file_system::Error),

    #[error("end of fat chain")]
    EndOfFatChain,

    #[error("invalid uft16 string encountered ({reason})")]
    InvalidUtf16String { reason: &'static str },

    #[error("invalid file name ({reason})")]
    InvalidFileName { reason: &'static str },

    #[error("invalid file system ({reason})")]
    InvalidFileSystem { reason: &'static str },

    #[error("file not found")]
    FileNotFound,

    #[error("directory not found")]
    DirectoryNotFound,

    #[error("invalid utf8 bytes ({0})")]
    Utf8Error(#[from] core::str::Utf8Error),

    #[error("disk is full")]
    DiskFull,
}

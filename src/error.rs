use super::{boot_sector, directory_entry, io::BlockDevice};

#[non_exhaustive]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(thiserror::Error, Debug)]
pub enum ExFatError<D: BlockDevice> {
    #[error("io")]
    Io(D::Error),

    #[error("directory entry ({0:?})")]
    DirectoryEntry(#[from] directory_entry::Error),

    #[error("boot sector not valid exFAT ({0:?})")]
    NotExFat(#[from] boot_sector::Error),

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

    #[error("invalid utf8 bytes")]
    Utf8Error,

    #[error("disk is full")]
    DiskFull,

    #[error("invalid cluster id ({0})")]
    InvalidClusterId(u32),

    #[error("invalid sector id ({0})")]
    InvalidSectorId(u32),

    #[error("directory is not empty")]
    DirectoryNotEmpty,

    #[error("write not enabled")]
    WriteNotEnabled,

    #[error("read not enabled")]
    ReadNotEnabled,

    #[error("file already exists")]
    AlreadyExists,

    #[error("cannot seek past the end of the valid data in the file")]
    SeekOutOfRange,

    #[error("attempt to change the allocation bitmap to a value with no effect")]
    InvalidAllocation,
}

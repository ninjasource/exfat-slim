// the file system exposed through shared references using the actor pattern and the Embassy async runtime

use core::sync::atomic::{AtomicU32, Ordering};

use alloc::{
    collections::btree_map::{BTreeMap, Entry},
    string::{String, ToString},
    vec,
    vec::Vec,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
    mutex::Mutex,
    semaphore::{FairSemaphore, Semaphore},
    signal::Signal,
};

use crate::asynchronous::{
    boot_sector, directory_entry,
    error::ExFatError,
    file::{File, Metadata, OpenOptions},
    file_system::{FileSystem, read_file_system_metadata},
    io::BlockDevice,
};

static REQ: Channel<CriticalSectionRawMutex, Req, 8> = Channel::new();

// to keep the API simple this error type is not generic
// as a result the BlockDevice error is turned into an error_code
// look for implementations of the ErrorCode trate to see what these codes mean in your codebase
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(thiserror::Error, Debug, Clone, Copy)]
pub enum Error {
    #[error("io error code {error_code}")]
    Io { error_code: u8 },

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

    #[error("the combination of flags set when opening the file is not valid")]
    InvalidOptions,

    #[error("unexpected response from file system actor")]
    UnexpectedResponse,

    #[error("no more file handles")]
    ExhaustedFileHandles,

    #[error("invalid file handle")]
    InvalidFileHandle,
}

impl<D> From<ExFatError<D>> for Error
where
    D: BlockDevice,
    D::Error: ErrorCode,
{
    fn from(value: ExFatError<D>) -> Self {
        match value {
            ExFatError::Io(e) => Error::Io {
                error_code: e.to_error_code(),
            },
            ExFatError::AlreadyExists => Error::AlreadyExists,
            ExFatError::DirectoryEntry(e) => Error::DirectoryEntry(e),
            ExFatError::DirectoryNotEmpty => Error::DirectoryNotEmpty,
            ExFatError::DirectoryNotFound => Error::DirectoryNotFound,
            ExFatError::DiskFull => Error::DiskFull,
            ExFatError::EndOfFatChain => Error::EndOfFatChain,
            ExFatError::FileNotFound => Error::FileNotFound,
            ExFatError::InvalidAllocation => Error::InvalidAllocation,
            ExFatError::InvalidClusterId(id) => Error::InvalidClusterId(id),
            ExFatError::InvalidFileName { reason } => Error::InvalidFileName { reason },
            ExFatError::InvalidFileSystem { reason } => Error::InvalidFileSystem { reason },
            ExFatError::InvalidOptions => Error::InvalidOptions,
            ExFatError::InvalidSectorId(id) => Error::InvalidSectorId(id),
            ExFatError::InvalidUtf16String { reason } => Error::InvalidUtf16String { reason },
            ExFatError::NotExFat(e) => Error::NotExFat(e),
            ExFatError::ReadNotEnabled => Error::ReadNotEnabled,
            ExFatError::SeekOutOfRange => Error::SeekOutOfRange,
            ExFatError::Utf8Error => Error::Utf8Error,
            ExFatError::WriteNotEnabled => Error::WriteNotEnabled,
        }
    }
}

impl OpenOptions {
    pub async fn open(&self, path: &str) -> Result<FileHandle, Error> {
        let token = ReplyPool::acquire().await;
        let req = Req {
            op: Op::OpenFile {
                path: path.to_string(),
                options: self.clone(),
            },
            reply: token,
        };
        REQ.send(req).await;
        let resp = ReplyPool::wait(token).await?;

        match resp {
            Resp::FileOpen { handle } => Ok(handle),
            _ => Err(Error::UnexpectedResponse),
        }
    }
}

enum Op {
    ReadToString { path: String },
    Read { path: String },
    OpenFile { path: String, options: OpenOptions },
    ReadFile { handle: FileHandle, len: usize },
    ReadFileToEnd { handle: FileHandle },
    ReadFileToString { handle: FileHandle },
    WriteFile { handle: FileHandle, buffer: Vec<u8> },
    SeekFile { handle: FileHandle, position: u64 },
    CloseFile { handle: FileHandle },
    FlushFile { handle: FileHandle },
    Metadata { handle: FileHandle },
}

struct Req {
    op: Op,
    reply: ReplyToken,
}

pub struct FileHandle(u32);

impl FileHandle {
    pub async fn read(&self, buf: &mut [u8]) -> Result<Option<usize>, Error> {
        let token = ReplyPool::acquire().await;
        let req = Req {
            op: Op::ReadFile {
                handle: FileHandle(self.0),
                len: buf.len(),
            },
            reply: token,
        };
        REQ.send(req).await;
        let resp = ReplyPool::wait(token).await?;

        match resp {
            Resp::Read { data } => {
                if data.is_empty() {
                    Ok(None)
                } else {
                    buf.copy_from_slice(&data);
                    Ok(Some(data.len()))
                }
            }
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub async fn read_to_end(&self) -> Result<Vec<u8>, Error> {
        let token = ReplyPool::acquire().await;
        let req = Req {
            op: Op::ReadFileToEnd {
                handle: FileHandle(self.0),
            },
            reply: token,
        };
        REQ.send(req).await;
        let resp = ReplyPool::wait(token).await?;

        match resp {
            Resp::Read { data } => Ok(data),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub async fn read_to_string(&self) -> Result<String, Error> {
        let token = ReplyPool::acquire().await;
        let req = Req {
            op: Op::ReadFileToString {
                handle: FileHandle(self.0),
            },
            reply: token,
        };
        REQ.send(req).await;
        let resp = ReplyPool::wait(token).await?;

        match resp {
            Resp::ReadToString { data } => Ok(data),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub async fn write(&self, buf: &[u8]) -> Result<(), Error> {
        let token = ReplyPool::acquire().await;
        let req = Req {
            op: Op::WriteFile {
                handle: FileHandle(self.0),
                buffer: buf.to_vec(),
            },
            reply: token,
        };
        REQ.send(req).await;
        let resp = ReplyPool::wait(token).await?;

        match resp {
            Resp::Ok => Ok(()),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub async fn seek(&self, position: u64) -> Result<(), Error> {
        let token = ReplyPool::acquire().await;
        let req = Req {
            op: Op::SeekFile {
                handle: FileHandle(self.0),
                position,
            },
            reply: token,
        };
        REQ.send(req).await;
        let resp = ReplyPool::wait(token).await?;

        match resp {
            Resp::Ok => Ok(()),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub async fn metdata(&self) -> Result<Metadata, Error> {
        let token = ReplyPool::acquire().await;
        let req = Req {
            op: Op::Metadata {
                handle: FileHandle(self.0),
            },
            reply: token,
        };
        REQ.send(req).await;
        let resp = ReplyPool::wait(token).await?;

        match resp {
            Resp::Metadata { data } => Ok(data),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    // this performs a flush and waits for the operation to complete, consuming file handle
    pub async fn close(self) -> Result<(), Error> {
        let token = ReplyPool::acquire().await;
        let req = Req {
            op: Op::CloseFile {
                handle: FileHandle(self.0),
            },
            reply: token,
        };
        REQ.send(req).await;
        let resp = ReplyPool::wait(token).await?;

        match resp {
            Resp::Ok => Ok(()),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    // this writes all outstanding metadata relating to changes to the file to disk
    // and returns when that operation is complete.
    // This includes file length (directory entry) FAT and allocation tables
    // The file is kept open
    pub async fn flush(&self) -> Result<(), Error> {
        let token = ReplyPool::acquire().await;
        let req = Req {
            op: Op::FlushFile {
                handle: FileHandle(self.0),
            },
            reply: token,
        };
        REQ.send(req).await;
        let resp = ReplyPool::wait(token).await?;

        match resp {
            Resp::Ok => Ok(()),
            _ => Err(Error::UnexpectedResponse),
        }
    }
}

impl Drop for FileHandle {
    // this will close the file but not wait for confirmation that the operation
    // was completed. If you want to be sure that the file was closed without error
    /// then call the close or flush functions explicitly
    fn drop(&mut self) {}
}

pub async fn open(path: &str, options: OpenOptions) -> Result<FileHandle, Error> {
    let token = ReplyPool::acquire().await;
    let req = Req {
        op: Op::OpenFile {
            path: path.to_string(),
            options,
        },
        reply: token,
    };
    REQ.send(req).await;
    let resp = ReplyPool::wait(token).await?;

    match resp {
        Resp::FileOpen { handle } => Ok(handle),
        _ => Err(Error::UnexpectedResponse),
    }
}

pub async fn read_to_string(path: &str) -> Result<String, Error> {
    let token = ReplyPool::acquire().await;
    let req = Req {
        op: Op::ReadToString {
            path: path.to_string(),
        },
        reply: token,
    };
    REQ.send(req).await;
    let resp = ReplyPool::wait(token).await?;

    match resp {
        Resp::ReadToString { data: string } => Ok(string),
        _ => Err(Error::UnexpectedResponse),
    }
}

pub async fn read(path: &str) -> Result<Vec<u8>, Error> {
    let token = ReplyPool::acquire().await;
    let req = Req {
        op: Op::Read {
            path: path.to_string(),
        },
        reply: token,
    };
    REQ.send(req).await;
    let resp = ReplyPool::wait(token).await?;

    match resp {
        Resp::Read { data } => Ok(data),
        _ => Err(Error::UnexpectedResponse),
    }
}

#[derive(Clone, Copy, Debug)]
struct ReplyToken {
    pub slot: u8,
    pub seq: u32,
}

struct ReplySignal {
    seq: u32,
    resp: Result<Resp, Error>,
}

const REPLY_SLOTS: usize = 4;

// Each slot carries (seq, Resp). seq prevents stale wakeups.
static REPLY_SIGNALS: [Signal<CriticalSectionRawMutex, ReplySignal>; REPLY_SLOTS] =
    [Signal::new(), Signal::new(), Signal::new(), Signal::new()];

// Semaphore limits number of in-flight requests to REPLY_SLOTS.
static FREE_SLOTS: FairSemaphore<CriticalSectionRawMutex, REPLY_SLOTS> =
    FairSemaphore::new(REPLY_SLOTS);

// Slot bitmap, protected by a Mutex (portable in Embassy).
static SLOT_BITMAP: Mutex<CriticalSectionRawMutex, u32> = Mutex::new(0);

// Monotonic sequence generator.
static SEQ: AtomicU32 = AtomicU32::new(1);

fn claim_slot_locked(bitmap: &mut u32) -> u8 {
    // Find a 0 bit and set it.
    for i in 0..REPLY_SLOTS {
        let mask = 1u32 << i;
        if (*bitmap & mask) == 0 {
            *bitmap |= mask;
            return i as u8;
        }
    }
    // Should be unreachable due to semaphore gating.
    0
}

fn release_slot_locked(bitmap: &mut u32, slot: u8) {
    let mask = 1u32 << (slot as u32);
    *bitmap &= !mask;
}

enum Resp {
    Ok,
    ReadToString { data: String },
    Read { data: Vec<u8> },
    FileOpen { handle: FileHandle },
    Metadata { data: Metadata },
}

struct Files {
    files: BTreeMap<u32, File>,
    next_handle: u32,
}

impl Files {
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            next_handle: 0,
        }
    }

    pub fn add(&mut self, file: File) -> Result<FileHandle, Error> {
        let mut counter: u32 = 0;
        loop {
            let handle = self.next_handle.wrapping_add(1);

            match self.files.entry(handle) {
                Entry::Occupied(_) => {
                    // file is still available here because it was not moved
                }
                Entry::Vacant(v) => {
                    v.insert(file); // moved only if key was absent
                    return Ok(FileHandle(handle));
                }
            }

            counter += 1;
            if counter == u32::MAX {
                return Err(Error::ExhaustedFileHandles);
            }
        }
    }

    pub fn remove(&mut self, handle: FileHandle) -> Result<File, Error> {
        self.files.remove(&handle.0).ok_or(Error::InvalidFileHandle)
    }

    pub fn get(&mut self, handle: &FileHandle) -> Result<&mut File, Error> {
        self.files
            .get_mut(&handle.0)
            .ok_or(Error::InvalidFileHandle)
    }
}

struct ReplyPool;

impl ReplyPool {
    /// Acquire a reply slot and create a token.
    pub async fn acquire() -> ReplyToken {
        // Ensure a slot is available.
        FREE_SLOTS.acquire(1).await.unwrap();

        // Claim an actual slot id.
        let slot = {
            let mut bm = SLOT_BITMAP.lock().await;
            claim_slot_locked(&mut bm)
        };

        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        ReplyToken { slot, seq }
    }

    /// Wait for completion of a specific token; releases the slot afterwards.
    pub async fn wait(token: ReplyToken) -> Result<Resp, Error> {
        let slot = token.slot as usize;

        loop {
            let ReplySignal { seq, resp } = REPLY_SIGNALS[slot].wait().await;
            if seq == token.seq {
                // Release slot back to pool.
                {
                    let mut bm = SLOT_BITMAP.lock().await;
                    release_slot_locked(&mut bm, token.slot);
                }
                FREE_SLOTS.release(1);
                return resp;
            }
            // If mismatched (rare; usually only after reset/reuse bugs), keep waiting.
        }
    }

    /// Actor-side: complete a request.
    pub fn complete(token: ReplyToken, resp: Result<Resp, Error>) {
        let signal = ReplySignal {
            seq: token.seq,
            resp,
        };
        REPLY_SIGNALS[token.slot as usize].signal(signal);
    }
}

pub trait ErrorCode {
    fn to_error_code(&self) -> u8;
}

pub async fn fs_actor_task<D>(mut device: D)
where
    D: BlockDevice,
    D::Error: ErrorCode,
{
    let rx = REQ.receiver();

    let metadata = loop {
        // there may be no sd card inserted so every request should reattempt to read the file system
        match read_file_system_metadata(&mut device).await {
            Ok(metadata) => break metadata,
            Err(e) => {
                let req = rx.receive().await;
                ReplyPool::complete(req.reply, Err(e.into()));
            }
        }
    };

    let mut file_system = FileSystem::new_inner(device, metadata);

    let mut files: Files = Files::new();

    loop {
        let Req { op, reply } = rx.receive().await;
        let resp = handle_req(op, &mut file_system, &mut files).await;

        // Complete the oneshot for this request.
        ReplyPool::complete(reply, resp);
    }
}

async fn handle_req<D>(
    op: Op,
    file_system: &mut FileSystem<D>,
    files: &mut Files,
) -> Result<Resp, Error>
where
    D: BlockDevice,
    D::Error: ErrorCode,
{
    let resp = match op {
        Op::ReadToString { path } => {
            let data = file_system.read_to_string(&path).await?;
            Resp::ReadToString { data }
        }
        Op::Read { path } => {
            let data = file_system.read(&path).await?;
            Resp::Read { data }
        }
        Op::OpenFile { path, options } => {
            let file = file_system.open(&path, options).await?;
            let handle = files.add(file).unwrap();
            Resp::FileOpen { handle }
        }
        Op::ReadFileToString { handle } => {
            let file = files.get(&handle)?;
            let data = file.read_to_string(file_system).await?;
            Resp::ReadToString { data }
        }
        Op::ReadFileToEnd { handle } => {
            let file = files.get(&handle)?;
            let mut data = Vec::new();
            file.read_to_end(file_system, &mut data).await?;
            Resp::Read { data }
        }
        Op::SeekFile { handle, position } => {
            let file = files.get(&handle)?;
            file.seek(file_system, position).await?;
            Resp::Ok
        }
        Op::ReadFile { handle, len } => {
            let file = files.get(&handle)?;
            let mut data = vec![0u8; len];
            file.read(file_system, &mut data).await?;
            Resp::Read { data }
        }
        Op::WriteFile { handle, buffer } => {
            let file = files.get(&handle)?;
            file.write(file_system, &buffer).await?;
            Resp::Ok
        }
        Op::CloseFile { handle } => {
            let file = files.remove(handle)?;
            file.close::<D>().await?;
            Resp::Ok
        }
        Op::FlushFile { handle } => {
            let file = files.get(&handle)?;
            file.flush::<D>().await?;
            Resp::Ok
        }
        Op::Metadata { handle } => {
            let file = files.get(&handle)?;
            let data = file.metadata();
            Resp::Metadata { data }
        }
    };

    Ok(resp)
}

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
    error::ExFatError,
    file::{File, Metadata, OpenOptions},
    file_system::FileSystem,
    io::BlockDevice,
};

static REQ: Channel<CriticalSectionRawMutex, Req, 8> = Channel::new();

#[derive(Debug, Clone, Copy)]
pub enum Error {
    NoCard,
    UnexpectedResponse,
    ExhaustedFileHandles,
    InvalidFileHandle,
}

pub enum Op {
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

pub struct Req {
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
        let resp = ReplyPool::wait(token).await;

        match resp {
            Resp::Read { data } => {
                if data.len() > 0 {
                    buf.copy_from_slice(&data);
                    Ok(Some(data.len()))
                } else {
                    Ok(None)
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
        let resp = ReplyPool::wait(token).await;

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
        let resp = ReplyPool::wait(token).await;

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
        let resp = ReplyPool::wait(token).await;

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
        let resp = ReplyPool::wait(token).await;

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
        let resp = ReplyPool::wait(token).await;

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
        let resp = ReplyPool::wait(token).await;

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
        let resp = ReplyPool::wait(token).await;

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
    let resp = ReplyPool::wait(token).await;

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
    let resp = ReplyPool::wait(token).await;

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
    let resp = ReplyPool::wait(token).await;

    match resp {
        Resp::Read { data } => Ok(data),
        _ => Err(Error::UnexpectedResponse),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReplyToken {
    pub slot: u8,
    pub seq: u32,
}

const REPLY_SLOTS: usize = 4;

// Each slot carries (seq, Resp). seq prevents stale wakeups.
static REPLY_SIGNALS: [Signal<CriticalSectionRawMutex, (u32, Resp)>; REPLY_SLOTS] =
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

pub enum Resp {
    Ok,
    Error,
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

    pub fn remove(&mut self, handle: FileHandle) -> Option<File> {
        self.files.remove(&handle.0)
    }

    pub fn get(&mut self, handle: &FileHandle) -> Option<&mut File> {
        self.files.get_mut(&handle.0)
    }
}

pub struct ReplyPool;

impl ReplyPool {
    /// Acquire a reply slot and create a token.
    pub async fn acquire() -> ReplyToken {
        // Ensure a slot is available.
        FREE_SLOTS.acquire(1).await.unwrap();

        // Claim an actual slot id.
        let slot = {
            let mut bm = SLOT_BITMAP.lock().await;
            claim_slot_locked(&mut *bm)
        };

        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        ReplyToken { slot, seq }
    }

    /// Wait for completion of a specific token; releases the slot afterwards.
    pub async fn wait(token: ReplyToken) -> Resp {
        let slot = token.slot as usize;

        loop {
            let (got_seq, resp) = REPLY_SIGNALS[slot].wait().await;
            if got_seq == token.seq {
                // Release slot back to pool.
                {
                    let mut bm = SLOT_BITMAP.lock().await;
                    release_slot_locked(&mut *bm, token.slot);
                }
                FREE_SLOTS.release(1);
                return resp;
            }
            // If mismatched (rare; usually only after reset/reuse bugs), keep waiting.
        }
    }

    /// Actor-side: complete a request.
    pub fn complete(token: ReplyToken, resp: Resp) {
        REPLY_SIGNALS[token.slot as usize].signal((token.seq, resp));
    }
}

pub async fn fs_actor_task<D: BlockDevice>(device: D) -> Result<(), ExFatError<D>> {
    let mut file_system = FileSystem::new(device).await?;
    let mut files: Files = Files::new();

    let rx = REQ.receiver();
    loop {
        let req = rx.receive().await;

        let resp = match req.op {
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
                if let Some(file) = files.get(&handle) {
                    let data = file.read_to_string(&mut file_system).await?;
                    Resp::ReadToString { data }
                } else {
                    Resp::Error
                }
            }
            Op::ReadFileToEnd { handle } => {
                if let Some(file) = files.get(&handle) {
                    let mut data = Vec::new();
                    file.read_to_end(&mut file_system, &mut data).await?;
                    Resp::Read { data }
                } else {
                    Resp::Error
                }
            }
            Op::SeekFile { handle, position } => {
                if let Some(file) = files.get(&handle) {
                    file.seek(&mut file_system, position).await?;
                    Resp::Ok
                } else {
                    Resp::Error
                }
            }
            Op::ReadFile { handle, len } => {
                if let Some(file) = files.get(&handle) {
                    let mut data = vec![0u8; len];
                    file.read(&mut file_system, &mut data).await?;
                    Resp::Read { data }
                } else {
                    Resp::Error
                }
            }
            Op::WriteFile { handle, buffer } => {
                if let Some(file) = files.get(&handle) {
                    file.write(&mut file_system, &buffer).await?;
                    Resp::Ok
                } else {
                    Resp::Error
                }
            }
            Op::CloseFile { handle } => {
                if let Some(file) = files.remove(handle) {
                    file.close().await?;
                    Resp::Ok
                } else {
                    Resp::Error
                }
            }
            Op::FlushFile { handle } => {
                if let Some(file) = files.get(&handle) {
                    file.flush().await?;
                    Resp::Ok
                } else {
                    Resp::Error
                }
            }
            Op::Metadata { handle } => {
                if let Some(file) = files.get(&handle) {
                    let data = file.metadata();
                    Resp::Metadata { data }
                } else {
                    Resp::Error
                }
            }
        };

        // Complete the oneshot for this request.
        ReplyPool::complete(req.reply, resp);
    }
}

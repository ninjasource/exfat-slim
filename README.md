# exfat-slim
An exFAT file system library written in safe Rust for embedded environments. 

## Introduction

The exfat file system is an upgrade to the FAT32 file system. 
The table below highlights some differences. 

| | exFAT | FAT32 |
|---|---|---|
| Max file size | 128 PB | 4 GiB - 1 byte |
| Max volume size | 128 PB | 2 TB |
| Max filename length | 255 chars | 255 chars |
| Filename encoding | UTF-16LE | Long filenames use 16-bit Unicode entries |
| Max files in one folder | 2,796,202 | ~65,534 in Windows FAT32 implementation; fewer with long names |
| Contiguous sector access `NoFatChain` | Yes for unfragmented files. Fat chain for fragmented files | Fat chain only

The file system is optimized for flash storage and is particularly good for SD cards when it comes to wear levelling.
This is a `[no_std]` implementation requiring the `alloc` crate.
There is no unsafe Rust in the codebase (excl. dependencies)

## Why build this

I wanted to build a `no_std` file system library with the same ergonomics of the Rust standard library. I also wanted to support both a blocking and async api.
There is no LLM generated code in this repo for two reasons. 
One is that I enjoy writing software (even the typing bit) and I wrote this code for the learning experience it gave me rather than getting things done.
Secondly, I respect the people who have to read my code and I personally find it easier to read a codebase when I can trust that it is hallucination free. 
I am not knocking the amazing coding agents out there right now, its just that it is difficult to trust what you see because not all llm agents do the same job of things.

## Usage

You will need to implement your own block device which is the piece of code that sends and receives blocks of data (usually 512 bytes) to and from your SD card, flash memory or hard drive. 
In the examples below I have created a simple in-memory block device for demonstration purposes. It is up to the user to synchronise access to the block device. 

Reading a file into a string:
```rust
let io = InMemoryBlockDevice::new(); // your SD card driver
let mut fs = FileSystem::new(io).await?;
let contents: String = fs
    .read_to_string("/temp2/hello2/shoe/test.txt")
    .await?;
```

Listing all files and folders in the folder "/temp2/hello2"
```rust
let io = InMemoryBlockDevice::new(); // your SD card driver
let mut fs = FileSystem::new(io).await?;

let path = "/temp2/hello2";
let mut dir = fs.read_dir(path).await?;
while let Some(entry) = dir.next_entry(&mut fs).await? {
    println!("{:?}", entry);
}
```

Open a file for writing, make some writes, change the file cursor, overwrite some data, read the contents
```rust
    let io = InMemoryBlockDevice::new();
    let mut fs = FileSystem::new(io).await?;
    let path = "/temp2/test7.txt";

    // create empty read write file
    let options = OpenBuilder::new()
        .write(true)
        .create(true)
        .truncate(true)
        .build()?;
    let mut file = fs.open(path, options).await?;
    file.write(b"hello").await?;
    file.write(b" world").await?;
    file.seek(6).await?;
    file.write(b"W").await?;

    let contents = fs.read_to_string(path).await?;
    println!("{contents}"); // hello World
```

## Caching

There is a built in sector cache that allows the system to correctly manage the allocation bitmap and the fat table at a global level. 
In addition to the data cache speeds up reads and writes, especially when the same sector keeps being accessed. 
The caching level can be adjusted when creating an instance of the file system. 
The default (N = 4) is reasonable for an embedded device, using 4 x 3 x 512 = 6144 bytes of ram.
You need to close or flush a file to make it durable (persisted to media).

Another point to make is that the volume dirty bit is never touched for write operations as it is not mandatory to do so in the spec.
It is, however, exposed in the `utils` module if you chose to use it manually.
Note that power loss during a large write operation could result in pre-allocated clusters being permanently lost (at least until a reformat of the disk) as well as data loss from data not written to disk.

## Usage guidelines

As per the license, this codebase comes with no guarantees and I don't accept any responsibility for data corruption or loss regardless of the guidelines that follow. 
Read functions do not mutate the file system so are considered safer than write functions if you are concerned about data corruption or loss. 
The library should not panic and if it does as a result of a badly formed file system then this is a bug, please report it. 
The only exception to this is `unimplemented!` or `todo!` code sections that are temporary. 
I have attempted to replicate how the Rust standard library exposes a file system so that the API feels familiar. 
As a result I have chosen to require an allocator. 
If you create a file in a nested directory the library will attempt to create all the required directories if they do not exist.

## Work in progress

My primary use-case for this library is for an embedded device using the Embassy async framework. 
Therefore it is async-first and I am initially only planning on implementing the bits I really need.
This project is still undergoing significant churn and testing so just be aware of that

Implemented so far:
- Read a file
- List all directory entries (files and directories)
- Check if file or directory exists
- Support for multiple nested directories
- Delete a file
- Write a file
- Create a directory
- Dual async and blocking support
- Rename a file or directory (equivalent to move if parent directories change)
- Delete an empty directory
- Copy a file
- Open File
    - Read
    - Write
    - Create
    - Create New
    - Appends
    - Truncate
    - Seek
- Use Actor pattern so that BlockDevice trait can take a shared reference to self
- Support `close()` and `flush()` 
- Support block caching for allocation bitmap, fat chain and data blocks (directory entries and files)
- Embassy example

Work in progress:
- Return zeros where user attempts to read past valid_data_length in file (see File::read function)
- Truncate to specified length to preallocate a file
- Better test coverage
- Timestamps
- Maintain list of locked open files
- Add support for different block sizes (currently only 512 byte blocks supported)
- Enable file `close()` on Drop when using the actor pattern

## Contribution

If you do want to contribute then I would appreciate raised issues (by humans only) instead of PRs at this point in the project. 
I expect a lot of churn in the near future and I don't want the codebase to get unwieldy before then. 
Also, vibe coding and agentic bots and have made the whole PR process unpleasant in recent times. 
At some point I will open it up but please respect my need to keep this as a portfolio project for now.

## Troubleshooting

If you are reading an SD card directly using a microcontroller you will discover that sector 0 on the card is the MBR (master boot record) or GPT (GUID partition table) and NOT the boot sector of the exfat file system. 
The MBR or GPT will point to the boot sector of the exfat file system. 
However, when you use `dd` to clone an sd card you normally get the bytes of the mounted file system so sector 0 is, indeed, the boot sector of the exFAT file system in that case.
The `sd.img.gz` is such a clone. 
It is zipped because it contains mostly zeros. You can use a crate like `mbr-nostd` to read the MBR of an sd card from a microcontroller.

## Assumptions and Limitations

Assume a block size of 512 bytes. Even though exFAT supports block sizes from 512 to 4096 bytes this library was primarily written with SD cards in mind and they use 512 byte blocks for now.
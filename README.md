# exfat-slim
An exFAT file system library written in safe Rust for embedded environments

## Introduction

The exfat file system is an upgrade to the FAT32 file system (which limits to 4GB max files size and 2TB max volume size).
In exFAT, the max volume size is 128 petabytes with max files sizes up to that.
The file system is optimized for flash storage and is particularly good for SD cards when it comes to wear levelling.
This is a `[no_std]` implementation requiring the `alloc` crate.

## Why build this

There are already multiple Rust implementations of the exFAT file system out there. 
However, I wanted to hand write a version with no unsafe Rust and to experiment with various API ideas I have in the space.
There is no LLM generated code in this repo for two reasons. 
One is that I enjoy writing software (even the typing bit) and I wrote this code for the learning experience it gave me rather than getting things done.
Secondly, I respect the people who have to read my code and I personally find it easier to read a codebase when I can trust that it is hallucination free. 
I am not knocking the amazing coding agents out there right now, its just that it is difficult to trust what you see because not all llm agents do the same job of things.

## Usage

You will need to implement your own block device which is the piece of code that sends and receives blocks of data (usually 512 bytes) to and from your SD card, flash memory or hard drive. In the examples below I have created a simple in-memory block device for demonstration purposes. Since this block device may require a shared bus (like SPI) I have chosen to make all `FileSystem` calls immutable so that it can be passed around easily and make the user pass in mutable block device for every call. Therefore it is up to the user to synchronise access to the block device. 

Reading a file into a string:
```rust
let mut io = InMemoryBlockDevice::new();
let fs = FileSystem::new(&mut io).await?;
let contents: String = fs
    .read_to_string(&mut io, "/temp2/hello2/shoe/test.txt")
    .await?;
```

Listing all files and folders in the root directory (or folder "/temp2/hello2")
```rust
let mut io = InMemoryBlockDevice::new();
let fs = FileSystem::new(&mut io).await?;

let path = ""; // or "/temp2/hello2"
let mut list: DirectoryIterator = fs.read_dir(&mut io, path).await?;
while let Some(item) = list.next(&mut io).await? {
    println!("{:?}", item);
}
```

## Usage guidelines

As per the license, this codebase comes with no guarantees and I don't accept any responsibility for data corruption or loss regardless of the guidelines that follow. Read functions do not mutate the file system so are considered safer than write functions if you are concerned about data corruption or loss. The library should not panic and if it does as a result of a badly formed file system then this is a bug, please report it. The only exception to this is `unimplemented!` or `todo!` code sections that are temporary. I have attempted to replicate how the Rust standard library exposes a file system so that the API feels familiar. As a result I have chosen to require an allocator. If you create a file in a nested directory the library will attempt to create all the required directories if they do not exist.

## Work in progress

My primary use-case for this library is for an embedded device using the Embassy async framework. 
Therefore it is async-first and I am initially only planning on implementing the bits I really need.
This project is still very much alpha quality so please be aware of that.

Implemented so far:
- Read a file
- List all directory entries (files and directories)
- Check if file or directory exists
- Support fo multiple nested directories
- Delete a file
- Write a file
- Create a direcory

Not yet implemented (soon though):
- Dual async and blocking support
- Appending to a file
- Delete an empty directory

No immediate intention of implementing:
- Seekable read write file because it requires long term state management and some form of global locking which would then have to be added to everything else.

## Contribution

If you do want to contribute then I would appreciate raised issues instead of PRs at this point in the project. 
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

Assume a block size of 512 bytes. Even though exFAT supports block sizes from 512 to 4096 bytes this library was primarily written with SD cards in mind and they use 512 byte blocks.
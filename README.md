# exfat-slim
An exFAT file system library written in safe Rust for embedded environments

## Introduction

The exfat file system is an upgrade to the FAT32 file system (which limits to 4GB max files size and 2TB max volume size).
In exFAT, the max volume size is 128 petabytes with max files sizes up to that.
The file system is optimized for flash storage and is particularly good for SD cards when it comes to wear leveling.
This is a `[no_std]` implementation requiring the `alloc` crate.

## Why build this

There are already multiple Rust implementations of the exfat file system out there. 
However, I wanted to hand write a version without any unsafe Rust and to experiment with various API ideas I have in the space.
There is no LLM generated code in this repo for two reasons. 
One is that I actually enjoy writing software and I wrote this code for the learning experience it gave me rather than getting things done.
Secondly, I respect the people who have to read my code and I personally find it easier to read a codebase when I can trust that it is hallucination free. 
I am not knocking the amazing coding agents out there right now, its just that it is difficult to trust what you see because not all llm agents do the same job of things.

## Usage

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


## Work in progress

My primary use-case for this library is for an embedded device using the Embassy async framework. 
Therefore it is async-first and I am initially only planning on implementing the bits I really need.
This project is still very much alpha quality so please be aware of that.

Implemented so far:
- Read a file
- List all directory entries
- Check if file or directory exists
- Multiple nested directories

Not yet implemented (soon though):
- Delete a file
- Delete a directory
- Write a file
- Create a direcory
- Dual async and blocking support

No immediate intention of implementing:
- Seekable read write file because it requires long term state management and some form of global locking which would then have to be added to everything else.

## Contribution

If you do want to contribute then I would appreciate raised issues instead of PRs at this point in the project. 
I expect a lot of churn in the near future and I don't want the codebase to get unwieldy before that point. 
Also, vibe coding bots and punters have made the whole PR process a pain in recent times. 
At some point I will open it up but please respect my need to keep this as a portfolio project for now.

## Troubleshooting

If you are reading an SD card directly using a microcontroller you will discover that sector 0 on the card is the MBR (master boot record) or GPT (GUID partition table) and not the boot sector of the exfat file system. 
The MBR or GPT will point to the first sector of the exfat file system. 
When you use `dd` to clone an sd card you normally get the bytes of the mounted file system so sector 0 is, indeed, the boot sector of the exFAT file system in that case.
The `sd.img.gz` is such a clone. 
It is zipped because it contains mostly zeros. You can use a crate like `mbr-nostd` to read the MBR of an sd card from a microcontroller.
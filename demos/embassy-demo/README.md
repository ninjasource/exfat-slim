# Embassy File System Demo

## Introduction

This embedded application targets an STM32U5G9ZJTxQ mcu and demonstrates the following:

1. Read and Write Async access to an SD card formatted to the exFAT file system
2. Defmt logging to a battery backed BACKUP RAM buffer and persisted to the SD card in block size chunks. Therefore panics can be persisted to disk.
3. Logging using data and time from a battery backed RTC

## How it works

Access to the file system is coordinated using the actor pattern. 
This means that you don't have to have exclusive assess to the file system to interact with it and you can make calls like this:

read a file:
```rust
let text = fs::read_to_string("hello.txt").await?;
```

and write a file:
```rust
fs::write("hello.txt", b"Hello, World!").await?;
```

If there is no SD card then a NoCard error will be returned. 
Removing and reinserting the card will result in it being reinitialized on first use (there is no need to manually mount and unmount the file system)
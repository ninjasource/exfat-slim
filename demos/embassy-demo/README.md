# Embassy File System Demo

## Introduction

This embedded application targets an STM32U5G9ZJTxQ mcu and demonstrates the following:

1. Read and Write Async access to an SD card formatted to the `exFAT` file system
2. Defmt logging to a battery backed BACKUP RAM buffer and persisted to the SD card.
   This allows panics to be persisted sometime after restart. 
   All file access is buffered to reduce SD card wear.
3. Logging using date and time from a battery backed RTC
4. Perform a daily reset that also flushes the logs to disk.

## How it works

Access to the file system is coordinated using the actor pattern. The `file_system_task` is spawned like so:
`spawner.spawn(unwrap!(file_system_task(sdmmc)));`

You don't have to have exclusive access to the file system to interact with it and you can make calls like this:

read a file:
```rust
let text = fs::read_to_string("hello.txt").await?;
```

and write a file:
```rust
fs::write("hello.txt", b"Hello, World!").await?;
```

If there is no SD card then a NoCard error will be returned. 
Removing and reinserting the card will result in it being reinitialized on first use.
There is no need to manually mount and unmount the file system.

## Read the logs

```bash
defmt-print -e ./target/thumbv8m.main-none-eabihf/release/embassy-demo.elf < log.bin
```
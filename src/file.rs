use alloc::{string::String, vec::Vec};
use core::str::from_utf8;

use super::{
    allocation::{AllocatedRun, StoredChain},
    allocation_bitmap::Allocation,
    bisync,
    directory_entry::{
        DirectoryEntryChain, FileAttributes, GeneralSecondaryFlags, Location, RAW_ENTRY_LEN,
        StreamExtensionDirEntry, update_checksum,
    },
    error::ExFatError,
    file_system::{FileSystem, write_dir_entries_to_disk},
    io::{BLOCK_SIZE, BlockDevice},
    utils::split_path,
};

#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub truncate: bool,
    pub create: bool,
    pub create_new: bool,
}

pub(crate) const NO_CLUSTER_ID: u32 = 0;

impl OpenOptions {
    pub const fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
        }
    }

    /// Set option for read access
    ///
    /// If read it true the file should be readable if opened
    pub const fn read(mut self, read: bool) -> Self {
        self.read = read;
        self
    }

    /// Set option for write access
    ///
    /// If write is true the file should be writable if opened
    pub const fn write(mut self, write: bool) -> Self {
        self.write = write;
        self
    }

    /// Sets the option for append mode
    ///
    /// If append is true then writes will append to a file instead of overwriting its contents
    /// Setting `.write(true).append(true)` has the same affect as only setting `.append(true)`
    /// This option does not create a file if it does not exist, use create or create_new for that
    pub const fn append(mut self, append: bool) -> Self {
        self.append = append;
        self
    }

    /// Sets the option to truncate the previous file
    ///
    /// If truncate is true, opening the file will truncate the file length to 0 if it already exists.
    /// The file must be opened with `.write(true)` for this to work.
    pub const fn truncate(mut self, truncate: bool) -> Self {
        self.truncate = truncate;
        self
    }

    /// Sets the option to create a new file or simply open it if it already exists
    ///
    /// In order for the file to be created either `.write(true)` or `.append(true)` must be used.
    /// Calling `.create()` without `.write()` or `append()` will return an error on open
    pub const fn create(mut self, create: bool) -> Self {
        self.create = create;
        self
    }

    /// Sets the option to create a new file and failing if it already exists
    ///
    /// In order for the file to be created either `.write(true)` or `.append(true)` must be used.
    /// If true `.create()` and `.truncate()` are ignored
    pub const fn create_new(mut self, create_new: bool) -> Self {
        self.create_new = create_new;
        self
    }

    /*
    pub fn build(&self) -> OpenOptions {
        OpenOptions {
            read: self.read,
            append: self.append,
            create: self.create,
            create_new: self.create_new,
            truncate: self.truncate,
            write: self.write,
        }
    }*/
}

#[derive(Debug, Clone)]
pub(crate) struct FileDetails {
    pub first_cluster: u32,
    pub data_length: u64,
    pub valid_data_length: u64, // number of valid bytes in the file (reads past valid_data_length should return zeros)
    pub attributes: FileAttributes,
    // #[allow(dead_code)]
    pub name: String, // TODO: look into removing this and only reading it if requested via an impl
    pub location: Location,
    pub flags: GeneralSecondaryFlags,
    pub secondary_count: u8,
}

#[cfg(feature = "defmt")]
impl defmt::Format for FileDetails {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(
            f,
            "FileDetails {{ first_cluster: {=u32}, data_length: {=u64}, valid_data_length: {=u64}, attributes: {}, location: {}, flags: {}, secondary_count: {=u8} }}",
            self.first_cluster,
            self.data_length,
            self.valid_data_length,
            self.attributes,
            self.location,
            self.flags,
            self.secondary_count,
        );
    }
}

impl FileDetails {
    pub fn get_num_clusters(&self, bytes_per_cluster: u32) -> usize {
        self.data_length.div_ceil(bytes_per_cluster as u64) as usize
    }
}

#[derive(Debug)]
pub struct Metadata {
    pub(crate) details: FileDetails,
}

// TODO: add created and modified timestamps here
impl Metadata {
    /// Size of the file in bytes
    pub fn len(&self) -> u64 {
        self.details.data_length
    }

    /// Returns true if the file contains zero bytes
    pub fn is_empty(&self) -> bool {
        self.details.data_length == 0
    }

    /// Returns true if the metadata is for a directory
    pub fn is_dir(&self) -> bool {
        self.details.attributes.contains(FileAttributes::Directory)
    }

    /// Returns true if the metadata is for a file (aka archive)
    pub fn is_file(&self) -> bool {
        self.details.attributes.contains(FileAttributes::Archive)
    }
}

pub struct File {
    pub(crate) details: FileDetails,
    current_cluster: u32,
    remaining_bytes_in_cluster: u32,
    cursor: u64,
    open_options: OpenOptions,
    chain: StoredChain,
}

impl File {
    pub(crate) fn new(
        file_details: &FileDetails,
        cluster_id: u32,
        cluster_length: u32,
        open_options: &OpenOptions,
        chain: StoredChain,
    ) -> Self {
        let cursor = if open_options.append {
            file_details.valid_data_length
        } else {
            0
        };

        let remaining_bytes_in_cluster = match &chain {
            StoredChain::Empty => 0,
            _ => cluster_length - (cursor % cluster_length as u64) as u32,
        };

        Self {
            details: file_details.clone(),
            current_cluster: cluster_id,
            remaining_bytes_in_cluster,
            cursor,
            open_options: open_options.clone(),
            chain,
        }
    }

    /// Gets the metadata about the file
    pub fn metadata(&self) -> Metadata {
        Metadata {
            details: self.details.clone(),
        }
    }

    #[bisync]
    pub async fn flush<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
    ) -> Result<(), ExFatError<D>> {
        fs.flush_cache().await?;
        Ok(())
    }

    #[bisync]
    pub async fn close<D: BlockDevice>(
        mut self,
        fs: &mut FileSystem<D>,
    ) -> Result<(), ExFatError<D>> {
        self.flush(fs).await?;
        Ok(())
    }

    /// Read bytes from file into buf and return the number of bytes read
    ///
    /// Read begins at the cursor position and ends at the lesser of the buf or file length
    #[bisync]
    pub async fn read<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        buf: &mut [u8],
    ) -> Result<Option<usize>, ExFatError<D>> {
        fs.mount().await?;
        if !self.open_options.read {
            return Err(ExFatError::ReadNotEnabled);
        }

        let remainder_in_file = self.details.valid_data_length - self.cursor;

        // check for end of file
        if self.eof() {
            return Ok(None);
        }

        self.next_cluster_if_required(fs).await?;
        let cluster_id = self.current_cluster;
        let cluster_offset = self.get_cluster_offset(fs);
        let start_sector_id = fs.fs.get_heap_sector_id(cluster_id)?;
        let sector_id = start_sector_id + cluster_offset / BLOCK_SIZE as u32;
        let sector_offset = cluster_offset as usize % BLOCK_SIZE;
        let remainder_in_sector = BLOCK_SIZE - sector_offset;

        // calculate max num bytes we can read
        let num_bytes = (remainder_in_sector as u64)
            .min(remainder_in_file)
            .min(buf.len() as u64) as usize;

        // read a single sector and copy the bytes into the user supplied buffer
        let mut block = [0u8; BLOCK_SIZE];
        fs.dev
            .read(sector_id, &mut block)
            .await
            .map_err(ExFatError::Io)?;
        buf[..num_bytes].copy_from_slice(&block[sector_offset..sector_offset + num_bytes]);

        // update file read cursor position
        self.move_file_cursor(num_bytes).await?;

        Ok(Some(num_bytes))
    }

    /// Read all bytes from file into the buffer, extending the buffer by the length of the file
    ///
    /// This behaves the same way the Rust std library equivalent function works
    /// If you only want the buf to contain file bytes then pass in an empty buf (length zero)
    /// If you pass in a non zero length buf the file bytes will be appended onto the end of the buf
    /// If you want to pass in preallocated memory then you are free to set the capacity of the buf passed in
    /// and the file will be copied from position 0 in the buf (if it is length 0)
    ///
    /// Exfat has the concept of valid_data_length which is less than or equal to data_length.
    /// If a zero length Vec is passed it will be extended to data_length size and the bytes between valid_data_length and data_length will contain zeros.
    #[bisync]
    pub async fn read_to_end<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        buf: &mut Vec<u8>,
    ) -> Result<usize, ExFatError<D>> {
        fs.mount().await?;
        let len = self.details.valid_data_length as usize;
        let valid_len = self.details.valid_data_length as usize;

        // fill empty space with zeros
        let start = buf.len();
        buf.resize(buf.len() + len, 0);

        // reading in block size chunks from position 0 is the most efficient way to get data off the disk in one go
        // we can ignore the len returned from the read operation as a result
        // we are only interested in reading valid_data_length bytes as the rest are garbage and we return zeros instead (initialized above)
        let (blocks, remainder) = buf[start..start + valid_len].as_chunks_mut::<BLOCK_SIZE>();
        for block in blocks {
            self.read(fs, block.as_mut_slice()).await?;
        }
        self.read(fs, remainder).await?;

        Ok(len)
    }

    /// Read all bytes from file and interprets them as a utf8 encoded string
    #[bisync]
    pub async fn read_to_string<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
    ) -> Result<String, ExFatError<D>> {
        fs.mount().await?;
        // because multi byte characters may cross sector boundaries
        // I recon its safer to read the entire file into a buffer before decoding it
        let mut buf = Vec::new();
        let len = self.read_to_end(fs, &mut buf).await?;
        let decoded = from_utf8(&buf[..len])
            .map_err(|_| ExFatError::Utf8Error)?
            .into();
        Ok(decoded)
    }

    fn should_convert_to_fat_chain(&self, run: &AllocatedRun) -> bool {
        match &self.chain {
            StoredChain::Empty => false,
            StoredChain::Contiguous {
                first,
                cluster_count: len,
            } => {
                let no_fat_chain = self
                    .details
                    .flags
                    .contains(GeneralSecondaryFlags::NoFatChain);

                no_fat_chain && (first + len + 1) != run.first_cluster
            }
            StoredChain::Fat {
                first: _first,
                last: _last,
                cluster_count: _len,
            } => false,
        }
    }

    #[bisync]
    async fn update_chain<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        run: &AllocatedRun,
    ) -> Result<(), ExFatError<D>> {
        if self.should_convert_to_fat_chain(run) {
            self.convert_file_to_fat_chain_if_required_new(fs).await?;
        }

        let has_fat_chain = !self
            .details
            .flags
            .contains(GeneralSecondaryFlags::NoFatChain);

        if has_fat_chain {
            let mut cluster_id = self.current_cluster;
            for cluster_next in run.first_cluster..run.first_cluster + run.cluster_count {
                fs.fat.set(cluster_id, cluster_next, &mut fs.dev).await?;
                cluster_id = cluster_next;
            }
        }

        let old_chain = self.chain.clone();

        match old_chain {
            StoredChain::Empty => {
                self.chain = StoredChain::Contiguous {
                    first: run.first_cluster,
                    cluster_count: run.cluster_count,
                };
                self.current_cluster = run.first_cluster;
                self.remaining_bytes_in_cluster = fs.fs.cluster_length;
            }
            StoredChain::Contiguous {
                first,
                cluster_count,
            } => {
                if has_fat_chain {
                    self.chain = StoredChain::Fat {
                        first,
                        last: run.first_cluster + run.cluster_count - 1,
                        cluster_count: cluster_count + run.cluster_count,
                    };
                } else {
                    self.chain = StoredChain::Contiguous {
                        first,
                        cluster_count: cluster_count + run.cluster_count,
                    }
                }
            }
            StoredChain::Fat {
                first,
                last: _last,
                cluster_count,
            } => {
                self.chain = StoredChain::Fat {
                    first,
                    last: run.first_cluster + run.cluster_count - 1,
                    cluster_count: cluster_count + run.cluster_count,
                }
            }
        }
        Ok(())
    }

    #[bisync]
    async fn allocate_clusters_for<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        num_bytes: usize,
    ) -> Result<(), ExFatError<D>> {
        let mut cluster_count = num_bytes.div_ceil(fs.fs.cluster_length as usize) as u32;

        loop {
            let run = fs
                .allocator
                .allocate(&mut fs.dev, &self.chain, cluster_count)
                .await?;
            self.update_chain(fs, &run).await?;
            cluster_count -= run.cluster_count;

            if cluster_count == 0 {
                return Ok(());
            }
        }
    }

    #[bisync]
    async fn next_cluster_if_required<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
    ) -> Result<(), ExFatError<D>> {
        if self.remaining_bytes_in_cluster == 0 {
            self.current_cluster = self.next_cursor_id(fs).await?;
            self.remaining_bytes_in_cluster = fs.fs.cluster_length;
        }

        Ok(())
    }
    /// Writes all bytes from buf into the file from the file cursor position
    ///
    /// This function will automatically increase the length of the file if necessary
    #[bisync]
    pub async fn write<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        buf: &[u8],
    ) -> Result<(), ExFatError<D>> {
        if buf.len() == 0 {
            return Ok(());
        }

        fs.mount().await?;
        if !self.open_options.write {
            return Err(ExFatError::WriteNotEnabled);
        }

        // keep track these file details to check if they have changed later
        let flags = self.details.flags;
        let valid_data_length = self.details.valid_data_length;
        let data_length = self.details.data_length;

        // allocate new clusters if required
        let used_in_cluster = (data_length % fs.fs.cluster_length as u64) as u32;
        let remaining_bytes_in_current_cluster = fs.fs.cluster_length - used_in_cluster;
        if buf.len() > remaining_bytes_in_current_cluster as usize {
            let num_bytes = buf.len() - remaining_bytes_in_current_cluster as usize;
            self.allocate_clusters_for(fs, num_bytes).await?;
        } else if data_length > 0 && used_in_cluster == 0 || self.current_cluster == NO_CLUSTER_ID {
            self.allocate_clusters_for(fs, fs.fs.cluster_length as usize)
                .await?;
        }
        self.update_data_length(buf.len());

        // write the first sector (could be partially full)
        self.next_cluster_if_required(fs).await?;
        let len = self.write_partial_sector_new(fs, buf).await?;

        // if there are still more bytes to write
        if len < buf.len() {
            let start_index = len;
            let (blocks, remainder) = buf[start_index..].as_chunks::<BLOCK_SIZE>();

            // write full sectors
            for block in blocks {
                self.next_cluster_if_required(fs).await?;
                let sector_id = self.get_current_sector_id(fs)?;
                fs.dev
                    .write(sector_id, block)
                    .await
                    .map_err(ExFatError::Io)?;
                self.move_file_cursor(block.len()).await?;
            }

            // write the last sector (could be partially full)
            let _len = self.write_partial_sector_new(fs, remainder).await?;
        }

        // check if we need to update the file directory entry
        if valid_data_length != self.details.valid_data_length
            || data_length != self.details.data_length
            || flags != self.details.flags
        {
            // read dir entries for this file from disk
            let mut dir_entries = self.get_file_dir_entry_set(fs).await?;

            // the stream ext is always the second entry
            let mut stream_ext: StreamExtensionDirEntry = (&dir_entries[1]).into();
            stream_ext.first_cluster = match &self.chain {
                StoredChain::Empty => NO_CLUSTER_ID,
                StoredChain::Contiguous {
                    first,
                    cluster_count: _cluster_count,
                } => *first,
                StoredChain::Fat {
                    first,
                    last: _last,
                    cluster_count: _cluster_count,
                } => *first,
            };
            stream_ext.data_length = self.details.data_length;
            stream_ext.valid_data_length = self.details.valid_data_length;
            stream_ext.general_secondary_flags = self.details.flags;

            // serialize the mutated stream ext back to the dir entry
            dir_entries[1].copy_from_slice(&stream_ext.serialize());

            // recalculate the file checksum and save back to appropriate dir entry
            update_checksum(&mut dir_entries);

            // write to disk - only the directory entries are written.
            write_dir_entries_to_disk(&mut fs.dev, self.details.location, dir_entries).await?;
        }

        Ok(())
    }

    /*
        /// Writes all bytes from buf into the file from the file cursor position
        ///
        /// This function will automatically increase the length of the file if necessary
        #[bisync]
        pub async fn write1<D: BlockDevice>(
            &mut self,
            fs: &mut FileSystem<D>,
            buf: &[u8],
        ) -> Result<(), ExFatError<D>> {
            if buf.len() == 0 {
                return Ok(());
            }

            fs.mount().await?;
            if !self.open_options.write {
                return Err(ExFatError::WriteNotEnabled);
            }

            // keep track these file details to check if they have changed later
            let flags = self.details.flags;
            let valid_data_length = self.details.valid_data_length;
            let data_length = self.details.data_length;

            self.update_data_length(buf.len());
            let has_fat_chain_original = !flags.contains(GeneralSecondaryFlags::NoFatChain);

            let (cluster_ids, has_fat_chain) = self.get_or_allocate_clusters(fs, buf.len()).await?;

            if !has_fat_chain_original && has_fat_chain {
                // if file had no fat chain before we set one up for existing data
                self.convert_file_to_fat_chain_if_required(fs).await?;
            }

            let mut cluster_ids = cluster_ids.into_iter();

            // write the first sector (could be partially full)
            let len = self.write_partial_sector(fs, buf, &mut cluster_ids).await?;

            // if there are still more bytes to write
            if len < buf.len() {
                let start_index = len;
                let (blocks, remainder) = buf[start_index..].as_chunks::<BLOCK_SIZE>();

                // write full sectors
                for block in blocks {
                    let sector_id = self.get_current_sector_id(fs)?;
                    fs.dev
                        .write(sector_id, block)
                        .await
                        .map_err(ExFatError::Io)?;
                    self.move_file_cursor_for_writes(fs, block.len(), &mut cluster_ids)?;
                }

                // write the last sector (could be partially full)
                let _len = self
                    .write_partial_sector(fs, remainder, &mut cluster_ids)
                    .await?;
            }

            // check if we need to update the file directory entry
            if valid_data_length != self.details.valid_data_length
                || data_length != self.details.data_length
                || flags != self.details.flags
            {
                // read dir entries for this file from disk
                let mut dir_entries = self.get_file_dir_entry_set(fs).await?;

                // the stream ext is always the second entry
                let mut stream_ext: StreamExtensionDirEntry = (&dir_entries[1]).into();
                stream_ext.data_length = self.details.data_length;
                stream_ext.valid_data_length = self.details.valid_data_length;
                stream_ext.general_secondary_flags = self.details.flags;

                // serialize the mutated stream ext back to the dir entry
                dir_entries[1].copy_from_slice(&stream_ext.serialize());

                // recalculate the file checksum and save back to appropriate dir entry
                update_checksum(&mut dir_entries);

                // write to disk - only the directory entries are written.
                write_dir_entries_to_disk(&mut fs.dev, self.details.location, dir_entries).await?;
            }

            Ok(())
        }
    */

    fn set_current_cluster<D: BlockDevice>(&mut self, cluster_id: u32, fs: &FileSystem<D>) {
        self.current_cluster = cluster_id;
        self.remaining_bytes_in_cluster = fs.fs.cluster_length;
        crate::info!(
            "set_current_cluster: cluster_id {} remaining_bytes_in_cluster {}",
            cluster_id,
            fs.fs.cluster_length
        );
    }

    /// Seek to an offset, in bytes, in the file
    #[bisync]
    pub async fn seek<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        cursor: u64,
    ) -> Result<(), ExFatError<D>> {
        fs.mount().await?;
        if cursor > self.details.valid_data_length {
            return Err(ExFatError::SeekOutOfRange);
        }

        self.cursor = cursor;
        let num_clusters = (cursor / fs.fs.cluster_length as u64) as u32;

        if self
            .details
            .flags
            .contains(GeneralSecondaryFlags::NoFatChain)
        {
            // no fat chain so all clusters are consecutive for this file
            self.set_current_cluster(self.details.first_cluster + num_clusters, fs);
        } else {
            self.set_current_cluster(self.details.first_cluster, fs);

            for _ in 0..num_clusters {
                match fs
                    .fat
                    .next_cluster_in_fat_chain(self.current_cluster, &mut fs.dev)
                    .await?
                {
                    Some(cluster_id) => self.current_cluster = cluster_id,
                    None => return Err(ExFatError::EndOfFatChain),
                }
            }
        }

        Ok(())
    }

    /// Copies a file from one file to another
    ///
    /// If directories in the to_path do not exist they will be created
    /// File attributes will also be copied but timestamps will be new
    #[bisync]
    pub(crate) async fn copy_to<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        to_path: &str,
    ) -> Result<(), ExFatError<D>> {
        let (dir_path, file_or_dir_name) = split_path(to_path);
        let num_clusters = self.num_clusters(fs);

        // find free space on the drive, preferring contiguous clusters
        let allocation = fs
            .alloc_bitmap
            .find_free_clusters(&mut fs.dev, &fs.fs, num_clusters, false, None)
            .await?;

        let run = fs
            .allocator
            .find_free_clusters(&mut fs.dev, num_clusters)
            .await?;

        if run.cluster_count != num_clusters {
            unimplemented!("writing to a file using the fat chain is not yet supported")
        }

        // find directory or recursively create it if it does not already exist
        let directory_cluster_id = fs.get_or_create_directory(dir_path).await?;

        let flags = GeneralSecondaryFlags::AllocationPossible | GeneralSecondaryFlags::NoFatChain;

        fs.create_file_dir_entry_at(
            file_or_dir_name,
            directory_cluster_id,
            run.first_cluster,
            self.details.attributes,
            flags,
            self.details.valid_data_length,
            self.details.data_length,
        )
        .await?;

        fs.allocator.mark_allocated(&mut fs.dev, &run, true).await?;

        let mut sector_id = fs.fs.get_heap_sector_id(run.first_cluster)?;
        let mut buf = [0u8; BLOCK_SIZE];

        while let Some(_len) = self.read(fs, &mut buf).await? {
            fs.dev
                .write(sector_id, &buf)
                .await
                .map_err(ExFatError::Io)?;
            sector_id += 1;
        }

        Ok(())
    }

    /*
    /// This function gets clusters (after the current_cluster) that can be used to write data to and ensures that clusters are all allocated
    /// If the cursor is at the end of the file (data_length) the function will allocate new clusters as required
    /// If the cursor is somewhere in the middle of the file it will return already allocated clusters
    /// If the custor is near the end of the file this function can return a combination of already
    ///   allocated clusters followed by newly allocated ones
    /// If the file flags are "no_fat_chain" and there are no more contiguous clusters
    ///   it will switch to using a fat chain and update the fat acordingly
    #[bisync]
    async fn get_or_allocate_clusters<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        num_bytes: usize,
    ) -> Result<(Vec<u32>, bool), ExFatError<D>> {
        if self.cursor > self.details.data_length {
            return Err(ExFatError::SeekOutOfRange);
        }

        //  let cluster = self.cursor / fs.fs.cluster_length as u64;
        //  let cluster_delta = (self.cursor + num_bytes as u64) / fs.fs.cluster_length as u64;

        // fast path, exit early
        //  if cluster == cluster_delta {
        //  defmt::info!("fast path, exit early");
        //      return Ok((Vec::new(), false));
        // }

        // we can fit num_bytes into the current cluster then return that cluster, no need to allocate more clusters
        //    let remaining_bytes_in_cluster =
        //        fs.fs.cluster_length - (self.cursor % fs.fs.cluster_length as u64) as u32;

        let remaining_bytes_in_cluster = self.remaining_bytes_in_cluster;

        // fast path, exit early
        if num_bytes <= remaining_bytes_in_cluster as usize {
            //  self.remaining_bytes_in_cluster -= num_bytes as u32;

            return Ok((Vec::new(), false));
        }

        crate::info!("get_or_allocate_clusters slow path");
        let mut allocated_cluster_ids = Vec::new();

        let (allocated_bytes, unallocated_bytes) = {
            let remaining =
                self.details.data_length - self.cursor - remaining_bytes_in_cluster as u64;
            let unallocated_bytes = remaining.min(num_bytes as u64) as usize;
            let allocated_bytes = num_bytes - unallocated_bytes;
            (allocated_bytes, unallocated_bytes)
        };

        crate::info!(
            "allocated_bytes: {} unallocated_bytes: {} current_cluster: {}",
            allocated_bytes,
            unallocated_bytes,
            self.current_cluster
        );

        // self.remaining_bytes_in_cluster -= allocated_bytes as u32;

        let mut has_fat_chain = !self
            .details
            .flags
            .contains(GeneralSecondaryFlags::NoFatChain);

        // add already allocated clusters (don't include the current cluster hence not using div_ceil)
        let num_allocated_clusters = allocated_bytes / (fs.fs.cluster_length as usize);

        let mut cluster_id = self.current_cluster;
        if has_fat_chain {
            // follow the fat chain and build up
            for _ in 0..num_allocated_clusters {
                if let Some(next_id) =
                    next_cluster_in_fat_chain(&mut fs.dev, fs.fs.fat_offset, cluster_id).await?
                {
                    cluster_id = next_id;
                    allocated_cluster_ids.push(cluster_id);
                } else {
                    return Err(ExFatError::EndOfFatChain);
                }
            }
        } else {
            // clusters are contiguous
            for _ in 0..num_allocated_clusters {
                cluster_id += 1;
                allocated_cluster_ids.push(cluster_id);
            }
        }

        if unallocated_bytes == 0 {
            return Ok((allocated_cluster_ids, has_fat_chain));
        }

        let num_unallocated_clusters =
            unallocated_bytes.div_ceil(fs.fs.cluster_length as usize) as u32;

        // returns all newly allocated clusters
        let allocation = fs
            .alloc_bitmap
            .find_free_clusters(
                &mut fs.dev,
                &fs.fs,
                num_unallocated_clusters,
                has_fat_chain,
                Some(self.current_cluster),
            )
            .await?;

        let cluster_ids = match allocation {
            Allocation::Contiguous {
                first_cluster,
                num_clusters,
            } => {
                let cluster_ids: Vec<u32> = (first_cluster..first_cluster + num_clusters).collect();
                cluster_ids
            }
            Allocation::FatChain { clusters } => {
                has_fat_chain = true;

                // set fat chain for newly allocated clusters
                let mut combined = vec![self.current_cluster];
                combined.extend_from_slice(&clusters);
                fat::update_fat_chain(&mut fs.dev, fs.fs.fat_offset, &combined).await?;
                clusters
            }
        };

        crate::info!("alloc about to call");
        fs.alloc_bitmap
            .mark_allocated(&mut fs.dev, &fs.fs, &cluster_ids, true)
            .await?;

        allocated_cluster_ids.extend_from_slice(&cluster_ids);
        Ok((allocated_cluster_ids, has_fat_chain))
    }*/

    fn update_data_length(&mut self, num_bytes: usize) {
        let valid_data_length =
            (self.cursor + num_bytes as u64).max(self.details.valid_data_length);
        self.details.data_length = valid_data_length.max(self.details.data_length);
        self.details.valid_data_length = valid_data_length;
    }

    fn get_current_sector_id<D: BlockDevice>(
        &self,
        fs: &mut FileSystem<D>,
    ) -> Result<u32, ExFatError<D>> {
        let cluster_offset_bytes = self.cursor % fs.fs.cluster_length as u64;
        let start_sector_id = fs.fs.get_heap_sector_id(self.current_cluster)?;
        let sector_id = start_sector_id + cluster_offset_bytes as u32 / BLOCK_SIZE as u32;
        Ok(sector_id)
    }

    fn num_clusters<D: BlockDevice>(&self, fs: &mut FileSystem<D>) -> u32 {
        self.details
            .data_length
            .div_ceil(fs.fs.cluster_length as u64) as u32
    }

    /// helps to convert a file that had no_fat_chain to one with a fat chain
    #[bisync]
    async fn convert_file_to_fat_chain_if_required_new<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
    ) -> Result<(), ExFatError<D>> {
        if self
            .details
            .flags
            .contains(GeneralSecondaryFlags::NoFatChain)
        {
            // unset the no_fat_chain flag
            self.details
                .flags
                .set(GeneralSecondaryFlags::NoFatChain, false);

            for cluster_id in self.details.first_cluster..self.current_cluster {
                fs.fat.set(cluster_id, cluster_id + 1, &mut fs.dev).await?;
            }
        }

        Ok(())
    }

    /*
    /// helps to convert a file that had no_fat_chain to one with a fat chain
    #[bisync]
    async fn convert_file_to_fat_chain_if_required<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
    ) -> Result<(), ExFatError<D>> {
        if self
            .details
            .flags
            .contains(GeneralSecondaryFlags::NoFatChain)
        {
            // unset the no_fat_chain flag
            self.details
                .flags
                .set(GeneralSecondaryFlags::NoFatChain, false);

            // turn cluster_id range in to cluster_id collection
            let cluster_ids: Vec<u32> =
                (self.details.first_cluster..self.current_cluster).collect();

            // update fat chain
            fat::update_fat_chain(&mut fs.dev, fs.fs.fat_offset, &cluster_ids).await?;
        }

        Ok(())
    }*/

    #[bisync]
    async fn write_partial_sector_new<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        buf: &[u8],
    ) -> Result<usize, ExFatError<D>> {
        if buf.is_empty() {
            return Ok(0);
        }

        let start_index = (self.cursor % BLOCK_SIZE as u64) as usize;
        let end_index = BLOCK_SIZE.min(start_index + buf.len());
        let sector_id = self.get_current_sector_id(fs)?;

        // for the first block, if the write does not start on a block boundary
        // we need to read the existing sector and add in the bit we want to write
        // for max efficiency the user should write in block size chunks
        if start_index > 0 || end_index < BLOCK_SIZE {
            let mut block = [0u8; BLOCK_SIZE];
            fs.dev
                .read(sector_id, &mut block)
                .await
                .map_err(ExFatError::Io)?;
            let len = end_index - start_index;
            block[start_index..end_index].copy_from_slice(&buf[..len]);
            fs.dev
                .write(sector_id, &block)
                .await
                .map_err(ExFatError::Io)?;
            self.move_file_cursor(len).await?;
            return Ok(len);
        }

        Ok(0)
    }

    /*
    #[bisync]
    async fn write_partial_sector<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        buf: &[u8],
        cluster_ids: &mut impl Iterator<Item = u32>,
    ) -> Result<usize, ExFatError<D>> {
        if buf.is_empty() {
            return Ok(0);
        }

        let start_index = (self.cursor % BLOCK_SIZE as u64) as usize;
        let end_index = BLOCK_SIZE.min(start_index + buf.len());
        let sector_id = self.get_current_sector_id(fs)?;

        // for the first block, if the write does not start on a block boundary
        // we need to read the existing sector and add in the bit we want to write
        // for max efficiency the user should write in block size chunks
        if start_index > 0 || end_index < BLOCK_SIZE {
            let mut block = [0u8; BLOCK_SIZE];
            fs.dev
                .read(sector_id, &mut block)
                .await
                .map_err(ExFatError::Io)?;
            let len = end_index - start_index;
            block[start_index..end_index].copy_from_slice(&buf[..len]);
            fs.dev
                .write(sector_id, &block)
                .await
                .map_err(ExFatError::Io)?;
            self.move_file_cursor_for_writes(fs, len, cluster_ids)?;
            return Ok(len);
        }

        Ok(0)
    }*/

    #[bisync]
    async fn get_file_dir_entry_set<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
    ) -> Result<Vec<[u8; RAW_ENTRY_LEN]>, ExFatError<D>> {
        let mut chain = DirectoryEntryChain::new_from_location(&self.details.location, &fs.fs);

        let mut counter = 0;

        let mut dir_entries = Vec::with_capacity(self.details.secondary_count as usize + 1);

        // copy all directory entries for the file into a Vec
        while let Some((dir_entry, _location)) = chain.next(fs).await? {
            let mut entry = [0u8; RAW_ENTRY_LEN];
            entry.copy_from_slice(dir_entry);
            dir_entries.push(entry);
            counter += 1;
            if counter == self.details.secondary_count + 1 {
                break;
            }
        }

        Ok(dir_entries)
    }

    fn get_cluster_offset<D: BlockDevice>(&self, fs: &mut FileSystem<D>) -> u32 {
        (self.cursor % fs.fs.cluster_length as u64) as u32
    }

    // end of file
    fn eof(&self) -> bool {
        self.cursor == self.details.data_length
    }

    /*
    #[bisync]
    async fn move_file_cursor_for_reads<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        num_bytes: usize,
    ) -> Result<(), ExFatError<D>> {
        self.cursor += num_bytes as u64;

        // assume that num_bytes is only ever up to the end of the current cluster
        // here we detect if we got to the end of the cluster and hence if we need to jump to the next cluster or not
        if num_bytes > 0 && self.cursor.is_multiple_of(fs.fs.cluster_length as u64) && !self.eof() {
            if self
                .details
                .flags
                .contains(GeneralSecondaryFlags::NoFatChain)
            {
                // no fat chain so all clusters are consecutive for this file
                self.set_current_cluster(self.current_cluster + 1, fs);
            } else if let Some(next_cluster_id) =
                next_cluster_in_fat_chain(&mut fs.dev, fs.fs.fat_offset, self.current_cluster)
                    .await?
            {
                self.set_current_cluster(next_cluster_id, fs);
            } else {
                return Err(ExFatError::EndOfFatChain);
            }
        }

        Ok(())
    }*/

    #[bisync]
    async fn move_file_cursor<D: BlockDevice>(
        &mut self,
        num_bytes: usize,
    ) -> Result<(), ExFatError<D>> {
        self.cursor += num_bytes as u64;
        self.remaining_bytes_in_cluster -= num_bytes as u32;
        Ok(())
    }

    #[bisync]
    async fn next_cursor_id<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
    ) -> Result<u32, ExFatError<D>> {
        match self.chain {
            StoredChain::Empty => Err(ExFatError::EndOfFatChain),
            StoredChain::Contiguous {
                first,
                cluster_count,
            } => {
                let cursor_id = self.current_cluster + 1;
                if (first..first + cluster_count).contains(&cursor_id) {
                    Ok(cursor_id)
                } else {
                    Err(ExFatError::EndOfFatChain)
                }
            }
            StoredChain::Fat {
                first: _first,
                last: _last,
                cluster_count: _len,
            } => {
                match fs
                    .fat
                    .next_cluster_in_fat_chain(self.current_cluster, &mut fs.dev)
                    .await?
                {
                    Some(cluster_id) => Ok(cluster_id),
                    None => Err(ExFatError::EndOfFatChain),
                }
            }
        }
    }

    /*
    fn move_file_cursor_for_writes<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        num_bytes: usize,
        cluster_ids: &mut impl Iterator<Item = u32>,
    ) -> Result<(), ExFatError<D>> {
        //        crate::info!(
        //            "move_file_cursor_for_writes: cursor {} num_bytes {} remaining_bytes_in_cluster {}",
        //            self.cursor,
        //            num_bytes,
        //            self.remaining_bytes_in_cluster
        //        );
        self.cursor += num_bytes as u64;

        if self.remaining_bytes_in_cluster >= num_bytes as u32 {
            self.remaining_bytes_in_cluster -= num_bytes as u32;
        } else {
            let num_bytes = num_bytes as u32 - self.remaining_bytes_in_cluster;
            self.remaining_bytes_in_cluster = 0;

            if let Some(cluster_id) = cluster_ids.next() {
                self.set_current_cluster(cluster_id, fs);
                self.remaining_bytes_in_cluster -= num_bytes;
            } else {
                return Err(ExFatError::EndOfFatChain);
            }
        }


        Ok(())
    }*/
}

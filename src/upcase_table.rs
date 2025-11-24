use crate::{
    directory_entry::UpcaseTableDirEntry, error::ExFatError, file_system::FileSystemDetails,
    io::BlockDevice,
};

#[derive(Debug)]
pub struct UpcaseTable {
    mapping: [u16; 128],
}

impl Default for UpcaseTable {
    fn default() -> Self {
        Self {
            mapping: [0u16; 128],
        }
    }
}

impl UpcaseTable {
    pub async fn load(
        &mut self,
        dir_entry: &UpcaseTableDirEntry,
        fs: &FileSystemDetails,
        io: &mut impl BlockDevice,
    ) -> Result<(), ExFatError> {
        let sector_id = fs.get_heap_sector_id(dir_entry.first_cluster)?;
        let block = io.read_sector(sector_id).await?;
        let (chars, _remainder) = block.as_chunks::<2>();
        for (to, from) in self.mapping.iter_mut().zip(chars) {
            *to = u16::from_le_bytes(*from);
        }

        Ok(())
    }

    pub fn upcase(&self, utf16_char: u16) -> u16 {
        let index = utf16_char as usize;

        if index < self.mapping.len() {
            self.mapping[index]
        } else {
            utf16_char
        }
    }
}

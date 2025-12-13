use crate::{
    directory_entry::UpcaseTableDirEntry, error::ExFatError, file_system::FileSystemDetails,
    io::BlockDevice,
};

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug)]
pub struct UpcaseTable {
    pub mapping: [u16; 128],
}

impl Default for UpcaseTable {
    fn default() -> Self {
        Self {
            mapping: [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43,
                44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64,
                65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85,
                86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74,
                75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 123, 124, 125, 126,
                127,
            ],
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

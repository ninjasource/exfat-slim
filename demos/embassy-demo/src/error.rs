use exfat_slim::asynchronous::fs;

#[derive(Debug, defmt::Format)]
pub enum Error {
    Fs(fs::Error),
}

impl From<fs::Error> for Error {
    fn from(value: fs::Error) -> Self {
        Self::Fs(value)
    }
}

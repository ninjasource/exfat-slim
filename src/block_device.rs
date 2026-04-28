// blocking only

use aligned::{Aligned, Alignment};

pub trait BlockDevice<const SIZE: usize> {
    /// The error type for the BlockDevice implementation.
    type Error: core::fmt::Debug;

    /// The alignment requirements of the block buffers.
    type Align: Alignment;

    /// Read one or more blocks at the given block address.
    fn read(
        &mut self,
        block_address: u32,
        data: &mut [Aligned<Self::Align, [u8; SIZE]>],
    ) -> Result<(), Self::Error>;

    /// Write one or more blocks at the given block address.
    fn write(
        &mut self,
        block_address: u32,
        data: &[Aligned<Self::Align, [u8; SIZE]>],
    ) -> Result<(), Self::Error>;

    /// Report the size of the block device in bytes.
    fn size(&mut self) -> Result<u64, Self::Error>;
}

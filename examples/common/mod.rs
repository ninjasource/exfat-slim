// NOTE I have intentionally duplicated code in this module in order to demonstrate
// that you do not need any special crates to use either asynchronous or blocking versions of this library.
// in fact, you can even use both at the same time. Not that you would.

#![allow(dead_code)]

use log::trace;

pub mod asynchronous;
pub mod blocking;

pub(crate) const EXAMPLE_EXFAT_IMAGE: &'static str = "./examples/common/sd.img.gz";

pub(crate) fn print(direction: &str, sector_id_with_offset: u32, sector_id: u32, is_cached: bool) {
    let cached = if is_cached { " (cached)" } else { "" };
    if sector_id >= 4096 {
        let cluster_id = ((sector_id - 4096) / 64) + 2;
        trace!(
            "{} sector {} cluster {}{}",
            direction, sector_id_with_offset, cluster_id, cached
        );
    } else {
        trace!("{} sector {}{}", direction, sector_id_with_offset, cached);
    }
}

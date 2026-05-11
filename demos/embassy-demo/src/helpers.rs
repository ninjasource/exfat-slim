pub const BLOCK_LEN: usize = 512;

pub struct LoggerHelper {
    remainder: usize, // the number of bytes left in the current file sector
}

impl LoggerHelper {
    pub fn new(file_len: u64) -> Self {
        let remainder = Self::calc_remainder(file_len);
        Self { remainder }
    }

    pub fn update_remainder(&mut self, len_a: usize, len_b: usize) {
        let num_bytes = len_a + len_b;

        // we dont care about the actual length of the file, only how it is alligned with the BLOCK_LEN
        let len = BLOCK_LEN - self.remainder + num_bytes;
        self.remainder = Self::calc_remainder(len as u64);
    }

    pub fn to_write(&mut self, len_a: usize, len_b: usize) -> Option<(usize, usize)> {
        let len = len_a + len_b;
        if len >= self.remainder {
            let a = len_a.min(self.remainder);
            let to_write_a = a;
            let mut to_write_b = 0;
            self.remainder -= a;

            if self.remainder > 0 {
                let b = len_b.min(self.remainder);
                to_write_b = b;
            }

            self.remainder = BLOCK_LEN;
            Some((to_write_a, to_write_b))
        } else {
            None
        }
    }

    const fn calc_remainder(file_len: u64) -> usize {
        BLOCK_LEN - (file_len % BLOCK_LEN as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn log_helper_none_when_less_than_block() {
        let mut helper = LoggerHelper::new(0);
        assert!(helper.to_write(100, 0).is_none());
    }

    #[test]
    pub fn log_helper_some_when_exactly_block() {
        let mut helper = LoggerHelper::new(0);
        let (a, b) = helper.to_write(BLOCK_LEN, 0).unwrap();
        assert_eq!(a, BLOCK_LEN);
        assert_eq!(b, 0);
    }

    #[test]
    pub fn log_helper_some_when_split_buffer() {
        let mut helper = LoggerHelper::new(0);
        let (a, b) = helper.to_write(BLOCK_LEN - 100, 100).unwrap();
        assert_eq!(a, BLOCK_LEN - 100);
        assert_eq!(b, 100);
    }

    #[test]
    pub fn log_helper_some_when_more_than_block() {
        let mut helper = LoggerHelper::new(0);
        let (a, b) = helper.to_write(BLOCK_LEN + 100, 100).unwrap();
        assert_eq!(a, BLOCK_LEN);
        assert_eq!(b, 0);
    }

    #[test]
    pub fn log_helper_none_when_multiple_less_than_block() {
        let mut helper = LoggerHelper::new(0);
        assert!(helper.to_write(100, 100).is_none());
        assert!(helper.to_write(0, 100).is_none());
    }

    #[test]
    pub fn log_helper_some_then_none_when_multiple_less_than_block() {
        let mut helper = LoggerHelper::new(0);
        let (a, b) = helper.to_write(BLOCK_LEN + 100, 100).unwrap();
        assert!(helper.to_write(0, 100).is_none());
    }

    #[test]
    pub fn log_helper_some_then_none_then_some() {
        let mut helper = LoggerHelper::new(0);
        helper.to_write(BLOCK_LEN + 100, 100); // BLOCK + 200
        helper.to_write(100, 200); // 300
        let (a, b) = helper.to_write(100, 415).unwrap();
        assert_eq!(a, 100);
        assert_eq!(b, 412);
    }

    #[test]
    pub fn log_helper_partial_file() {
        let mut helper = LoggerHelper::new(100);
        let (a, b) = helper.to_write(312, 200).unwrap();
        assert_eq!(a, 312);
        assert_eq!(b, 100);

        // once the partial file has been dealt with we can work with full blocks therafter
        let (a, b) = helper.to_write(412, 100).unwrap();
        assert_eq!(a, 412);
        assert_eq!(b, 100);
    }
}

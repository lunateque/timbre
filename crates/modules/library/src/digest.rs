//! A 128-bit FNV-1a digest.
//!
//! Folder hashes and signatures are stored in the database and compared with
//! the values written by an earlier build, so the function must not change
//! between compiler releases. `DefaultHasher` gives no such guarantee, and
//! XXH3 lives behind the edge crates, which a module may not depend on.

use std::path::Path;

const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

#[derive(Clone, Copy)]
pub(crate) struct Digest(u128);

impl Default for Digest {
    fn default() -> Self {
        Self(OFFSET)
    }
}

impl Digest {
    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u128::from(*byte)).wrapping_mul(PRIME);
        }
        // Length separates "ab" + "c" from "a" + "bc".
        self.0 ^= u128::try_from(bytes.len()).unwrap_or(u128::MAX);
    }

    pub(crate) fn text(&mut self, text: &str) {
        self.bytes(text.as_bytes());
    }

    pub(crate) fn path(&mut self, path: &Path) {
        self.bytes(&crate::store::path_to_bytes(path));
    }

    pub(crate) fn number(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    pub(crate) fn finish(self) -> String {
        format!("{:032x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Digest;

    #[test]
    fn distinct_inputs_differ_and_the_value_is_fixed() {
        let mut one = Digest::default();
        one.text("ab");
        one.text("c");
        let mut two = Digest::default();
        two.text("a");
        two.text("bc");
        assert_ne!(one.finish(), two.finish());
        let mut fixed = Digest::default();
        fixed.text("timbre");
        assert_eq!(fixed.finish(), "d9087f3f223c64bf6f3c7f289e71c90a");
    }
}

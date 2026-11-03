use fnv::FnvHasher;
use std::hash::Hasher;

#[derive(Default)]
pub struct StableHasher(FnvHasher);

impl StableHasher {
    pub fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    pub fn finish(&self) -> u64 {
        self.0.finish()
    }
}

pub fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hasher = StableHasher::default();
    hasher.write(bytes);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::stable_hash;

    #[test]
    fn stable_hash_keeps_its_documented_fnv_value() {
        assert_eq!(stable_hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}

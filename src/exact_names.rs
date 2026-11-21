//! Catalog dictionaries split by a reversible, exact short-string encoding.
//! Unsupported names retain the original full-byte dictionary semantics.
use crate::{inline_person, Map};

#[derive(Default)]
pub struct ExactNames<'a> {
    inline: Map<u64, u32>,
    fallback: Map<&'a [u8], u32>,
}

impl<'a> ExactNames<'a> {
    pub fn insert(&mut self, name: &'a [u8], value: u32) -> Option<u32> {
        match inline_person::encode(name) {
            Some(key) => self.inline.insert(key, value),
            None => self.fallback.insert(name, value),
        }
    }

    #[inline]
    pub fn get(&self, name: &[u8]) -> Option<&u32> {
        match inline_person::encode(name) {
            Some(key) => self.inline.get(&key),
            None => self.fallback.get(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_exact_name_domain_preserves_lookup_and_replacement() {
        let mut names = vec![
            Vec::new(),
            b" product".to_vec(),
            b"product ".to_vec(),
            b"product".to_vec(),
            b"Product".to_vec(),
            b"1234567890".to_vec(),
            b"long-product-\xCE\xA9".to_vec(),
        ];
        for len in 1..=10 {
            for byte in [0, b'0', b'1', b'A', 127, 128, 255] {
                names.push(vec![byte; len]);
            }
        }
        let mut exact = ExactNames::default();
        let mut reference = Map::default();
        for (index, name) in names.iter().enumerate() {
            assert_eq!(
                exact.insert(name, index as u32),
                reference.insert(name.as_slice(), index as u32)
            );
        }
        for (index, name) in names.iter().enumerate() {
            assert_eq!(exact.get(name), reference.get(name.as_slice()));
            assert_eq!(
                exact.insert(name, index as u32 + 1000),
                reference.insert(name.as_slice(), index as u32 + 1000)
            );
            let mut absent = name.clone();
            absent.push(1);
            assert_eq!(exact.get(&absent), reference.get(absent.as_slice()));
        }
    }
}

//! Exact event-ID tokens: reversible short IDs, interned fallback names.
use crate::{inline_person, Map};
const TAG_MASK: u64 = 0xf000_0000_0000_0000;
const NAMED: u64 = 0x4000_0000_0000_0000;
const LOCAL: u64 = 0x6000_0000_0000_0000;
const PAYLOAD_MASK: u64 = 0x0fff_ffff_ffff_ffff;

pub struct LocalEventIds<'a> {
    pub names: Vec<&'a [u8]>,
    ids: Map<&'a [u8], u64>,
    worker: u64,
    worker_bits: u32,
}
impl<'a> LocalEventIds<'a> {
    pub fn new(worker: usize, worker_bits: u32) -> Self {
        assert!(worker_bits < 60);
        assert!((worker as u64) < (1u64 << worker_bits));
        Self { names: Vec::new(), ids: Map::default(), worker: worker as u64, worker_bits }
    }
    #[inline]
    pub fn intern(&mut self, name: &'a [u8]) -> u64 {
        if let Some(id) = inline_person::encode(name) { return id; }
        if let Some(&id) = self.ids.get(name) { return id; }
        let local = u64::try_from(self.names.len()).expect("event-ID dictionary exceeds u64 capacity");
        assert!(local <= (PAYLOAD_MASK >> self.worker_bits), "event-ID token exceeds capacity");
        let id = LOCAL | (local << self.worker_bits) | self.worker;
        self.names.push(name);
        self.ids.insert(name, id);
        id
    }
}

#[derive(Default)]
pub struct EventIdRemaps {
    workers: Vec<Vec<u64>>,
    worker_bits: u32,
    worker_mask: u64,
}
impl EventIdRemaps {
    pub fn merge(workers: Vec<Vec<&[u8]>>, worker_bits: u32) -> Self {
        use std::collections::hash_map::Entry;
        let mut names: Map<&[u8], u64> = Map::default();
        let remaps = workers.into_iter().map(|worker| worker.into_iter().map(|name| {
            let next = u64::try_from(names.len()).expect("global event-ID dictionary exceeds u64 capacity");
            assert!(next <= PAYLOAD_MASK, "global event-ID token exceeds capacity");
            match names.entry(name) {
                Entry::Occupied(entry) => *entry.get(),
                Entry::Vacant(entry) => { let id = NAMED | next; entry.insert(id); id }
            }
        }).collect()).collect();
        Self { workers: remaps, worker_bits, worker_mask: (1u64 << worker_bits)-1 }
    }
    #[inline]
    pub fn canonical(&self, id: u64) -> u64 {
        if id & TAG_MASK != LOCAL { return id; }
        let payload = id & PAYLOAD_MASK;
        self.workers[(payload & self.worker_mask) as usize][(payload >> self.worker_bits) as usize]
    }
    #[inline]
    pub fn equal(&self, left: u64, right: u64) -> bool {
        left == right || self.canonical(left) == self.canonical(right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_tokens_preserve_names_across_workers() {
        let mut a=LocalEventIds::new(0,2);let mut b=LocalEventIds::new(3,2);
        let long=b"1234567890";let zero=b"123456789\0";
        let shared_a=a.intern(long);let shared_b=b.intern(long);
        let distinct=b.intern(zero);let unicode=a.intern("Ω".as_bytes());
        assert_ne!(shared_a,shared_b);assert_ne!(shared_b,distinct);
        assert_eq!(a.intern(long),shared_a);
        let remaps=EventIdRemaps::merge(vec![a.names,vec![],vec![],b.names],2);
        assert!(remaps.equal(shared_a,shared_b));assert!(!remaps.equal(shared_a,distinct));
        assert!(!remaps.equal(shared_a,unicode));
        assert_ne!(remaps.canonical(shared_a),inline_person::encode(b"123456789").unwrap());
    }
    #[test]
    fn all_inline_tokens_remain_disjoint_from_tags_including_u64_max() {
        let mut local=LocalEventIds::new(0,0);
        for len in 1..=9 {for byte in 0..128u8 {
            let bytes=vec![byte;len];let key=inline_person::encode(&bytes).unwrap();
            assert_ne!(key&TAG_MASK,NAMED);assert_ne!(key&TAG_MASK,LOCAL);
        }}
        assert_eq!(local.intern(&[127;9]),u64::MAX);
        assert_eq!(local.intern(b"a\0\r"),inline_person::encode(b"a\0\r").unwrap());
        assert!(local.names.is_empty());
        assert!(EventIdRemaps::default().equal(u64::MAX,u64::MAX));
    }
}

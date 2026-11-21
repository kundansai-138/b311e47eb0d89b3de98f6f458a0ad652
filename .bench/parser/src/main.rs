//! Exact native CSV, session, attribution, and Parquet engine.
mod event_ids;
mod inline_person;
mod parquet_output;
mod timestamp_parser;
use event_ids::{EventIdName, EventIdRemaps, LocalEventIds};
use parquet_output::{CampaignWriter, UserWriter};
use std::collections::HashMap;
use std::fs::{self, File};
use std::hash::{BuildHasherDefault, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;
use timestamp_parser::TimestampParser;

#[cfg(feature = "fast-csv")]
mod fast_csv;
#[cfg(feature = "row-csv")]
mod row_csv;

const DAY: i64 = 86_400_000;
const WEEK: i64 = 7 * DAY;
const GAP: i64 = 1_800_000;
const CHUNK: usize = 4 * 1024 * 1024;
type Map<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

// Hashes only choose buckets and accelerate ordering. Inline tokens encode
// original bytes injectively; fallback dictionaries compare complete bytes.
// Exact token identity, never a hash match alone, decides event equality.
#[derive(Default)]
struct FastHasher(u64);
impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0 = self.0.rotate_left(17) ^ hash_bytes(bytes);
    }
    #[cfg(feature = "fast-hash")]
    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.0 = self.0.rotate_left(17) ^ mix64(value as u64 ^ 0xa0761d6478bd642f);
    }
    #[cfg(feature = "fast-hash")]
    #[inline]
    fn write_u64(&mut self, value: u64) {
        // Exact inline IDs are already integers: hash their bits directly,
        // without converting to a byte slice and entering the string hash.
        self.0 = self.0.rotate_left(17) ^ mix64(value ^ 0xa0761d6478bd642f);
    }
}
#[cfg(not(feature = "fast-hash"))]
#[inline]
fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h ^= h >> 32;
    h = h.wrapping_mul(0xd6e8feb86659fd93);
    h ^ (h >> 32)
}

#[inline]
fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}
#[cfg(feature = "fast-hash")]
#[inline]
fn hash_bytes(bytes: &[u8]) -> u64 {
    // All bytes contribute. The hash only selects buckets; original byte
    // strings still decide equality, so collisions never merge distinct IDs.
    let mut h = 0x9e3779b97f4a7c15 ^ bytes.len() as u64;
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes(chunk.try_into().unwrap());
        h = (h ^ word).wrapping_mul(0x9e3779b185ebca87).rotate_left(27);
    }
    let tail = chunks.remainder();
    let tail_word = if tail.len() >= 4 {
        let a = u32::from_le_bytes(tail[..4].try_into().unwrap()) as u64;
        let b = u32::from_le_bytes(tail[tail.len() - 4..].try_into().unwrap()) as u64;
        (a << 32) | b
    } else if !tail.is_empty() {
        (tail[0] as u64) << 16 | (tail[tail.len() / 2] as u64) << 8 | tail[tail.len() - 1] as u64
    } else {
        0
    };
    mix64(h ^ tail_word)
}

#[cfg(all(feature = "fast-csv", not(feature = "row-csv")))]
#[inline]
fn rows<const N: usize>(bytes: &[u8]) -> fast_csv::Rows<'_, N> {
    fast_csv::Rows::new(bytes)
}
#[cfg(feature = "row-csv")]
#[inline]
fn rows<const N: usize>(bytes: &[u8]) -> row_csv::Rows<'_, N> {
    row_csv::Rows::new(bytes)
}
#[cfg(not(feature = "fast-csv"))]
#[inline]
fn rows<const N: usize>(bytes: &[u8]) -> impl Iterator<Item = [&[u8]; N]> {
    bytes.split(|&b| b == b'\n').filter_map(fields::<N>)
}

#[cfg(any(test, not(feature = "fast-csv")))]
#[inline]
fn field(value: &[u8]) -> Option<&[u8]> {
    if value.first() == Some(&b'"') {
        if value.len() < 2 || value.last() != Some(&b'"') {
            return None;
        }
        let inner = &value[1..value.len() - 1];
        if inner.contains(&b'"') {
            None
        } else {
            Some(inner)
        }
    } else if value.contains(&b'"') {
        None
    } else {
        Some(value)
    }
}
#[cfg(any(test, not(feature = "fast-csv")))]
#[inline]
fn fields<const N: usize>(line: &[u8]) -> Option<[&[u8]; N]> {
    if line.is_empty() {
        return None;
    }
    let mut result = [&[][..]; N];
    let mut parts = line.split(|&b| b == b',');
    for slot in &mut result {
        *slot = field(parts.next()?)?;
    }
    if parts.next().is_some() {
        None
    } else {
        Some(result)
    }
}
fn body(bytes: &[u8]) -> &[u8] {
    bytes
        .iter()
        .position(|&b| b == b'\n')
        .map_or(&[], |p| &bytes[p + 1..])
}
#[cfg(not(feature = "fast-digits"))]
#[inline]
fn digits(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return None;
    }
    let mut n = 0i64;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((b - b'0') as i64)?;
    }
    Some(n)
}
#[cfg(feature = "fast-digits")]
#[inline]
fn digits(mut bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return None;
    }
    // Arbitrarily many leading zeroes are legal. Once removed, at most19
    // decimal digits can fit i64;19 digits always fit the u64 accumulator.
    while bytes.first() == Some(&b'0') {
        bytes = &bytes[1..];
    }
    if bytes.len() > 19 {
        return None;
    }
    let mut n = 0u64;
    for &byte in bytes {
        let digit = byte.wrapping_sub(b'0');
        if digit > 9 {
            return None;
        }
        n = n * 10 + digit as u64;
    }
    (n <= i64::MAX as u64).then_some(n as i64)
}

#[inline]
fn pair(b: &[u8], i: usize) -> Option<i64> {
    let a = b[i];
    let c = b[i + 1];
    if a.is_ascii_digit() && c.is_ascii_digit() {
        Some(((a - b'0') as i64) * 10 + (c - b'0') as i64)
    } else {
        None
    }
}
fn civil_days(mut y: i64, m: i64, d: i64) -> i64 {
    y -= i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}
#[inline]
fn timestamp(b: &[u8]) -> Option<i64> {
    if b.len() != 20 && b.len() != 25 {
        return digits(b);
    }
    // A 20- or 25-digit F3 is still an F3, not an ISO timestamp.
    if b.get(4) != Some(&b'-') {
        return digits(b);
    }
    if b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y = digits(&b[..4])?;
    let m = pair(b, 5)?;
    let d = pair(b, 8)?;
    let h = pair(b, 11)?;
    let min = pair(b, 14)?;
    let sec = pair(b, 17)?;
    if y == 0 || !(1..=12).contains(&m) || h > 23 || min > 59 || sec > 59 {
        return None;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let max = match m {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if d < 1 || d > max {
        return None;
    }
    let offset = if b.len() == 20 {
        if b[19] != b'Z' {
            return None;
        }
        0
    } else {
        if (b[19] != b'+' && b[19] != b'-') || b[22] != b':' {
            return None;
        }
        let oh = pair(b, 20)?;
        let om = pair(b, 23)?;
        if oh > 23 || om > 59 {
            return None;
        }
        (oh * 60 + om) * 60 * 1000 * if b[19] == b'+' { 1 } else { -1 }
    };
    Some(civil_days(y, m, d) * DAY + (h * 3600 + min * 60 + sec) * 1000 - offset)
}
fn json_time(text: &str, key: &str) -> io::Result<i64> {
    let token = format!("\"{key}\"");
    let after = text
        .split_once(&token)
        .and_then(|(_, s)| s.split_once(':'))
        .map(|(_, s)| s.trim_start());
    let value = after
        .and_then(|s| s.strip_prefix('"'))
        .and_then(|s| s.split_once('"'))
        .map(|(s, _)| s);
    value
        .and_then(|s| timestamp(s.as_bytes()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid manifest timestamp"))
}

fn csv_paths(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.is_file() && path.extension().is_some_and(|s| s == "csv") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}
// Input datasets are immutable for the lifetime of this process. Read-only
// maps keep their backing storage alive until every borrowed field is dropped.
enum InputFile {
    Empty,
    Mapped(memmap2::Mmap),
}
impl std::ops::Deref for InputFile {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Empty => &[],
            Self::Mapped(map) => map,
        }
    }
}
fn load_file(path: &Path) -> io::Result<InputFile> {
    let file = File::open(path)?;
    if file.metadata()?.len() == 0 {
        return Ok(InputFile::Empty);
    }
    // SAFETY: the scorer mounts /data read-only and does not change these files.
    // The mapping owns the lifetime of all slices returned from this InputFile.
    let map = unsafe { memmap2::MmapOptions::new().populate().map(&file)? };
    Ok(InputFile::Mapped(map))
}
fn load_files(paths: &[PathBuf], ncpu: usize) -> io::Result<Vec<InputFile>> {
    let next = AtomicUsize::new(0);
    let results: Vec<_> = (0..paths.len()).map(|_| Mutex::new(None)).collect();
    thread::scope(|s| {
        for _ in 0..ncpu.min(paths.len()) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= paths.len() {
                    break;
                }
                *results[i].lock().unwrap() = Some(load_file(&paths[i]));
            });
        }
    });
    results
        .into_iter()
        .map(|m| m.into_inner().unwrap().unwrap())
        .collect()
}
fn chunks(files: &[InputFile]) -> Vec<&[u8]> {
    let mut result = Vec::new();
    for file in files {
        let mut rest = body(file);
        while !rest.is_empty() {
            let mut end = rest.len().min(CHUNK);
            if end < rest.len() {
                end = rest[end..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(rest.len(), |p| end + p + 1);
            }
            result.push(&rest[..end]);
            rest = &rest[end..];
        }
    }
    result
}

struct Identity<'a> {
    // Inline strings use exact integer keys, not hashes. The byte-keyed map
    // contains only names the inline codec cannot represent.
    ids: Map<&'a [u8], u64>,
    inline_ids: Map<u64, u64>,
    names: Vec<&'a [u8]>,
}
fn root(parents: &mut [usize], mut i: usize) -> usize {
    while parents[i] != i {
        parents[i] = parents[parents[i]];
        i = parents[i];
    }
    i
}
fn identities(files: &[InputFile]) -> Identity<'_> {
    use std::collections::hash_map::Entry;
    let reserve = files
        .iter()
        .map(|f| {
            body(f)
                .iter()
                .filter(|&&b| b == b'\n')
                .count()
                .saturating_mul(2)
        })
        .sum::<usize>();
    let mut ids = Map::default();
    let mut inline_ids = Map::with_capacity_and_hasher(reserve, Default::default());
    let mut names = Vec::with_capacity(reserve);
    let mut parents = Vec::with_capacity(reserve);
    let mut sizes = Vec::with_capacity(reserve);
    let mut minima = Vec::with_capacity(reserve);
    for data in files {
        for [a, b] in rows::<2>(body(data)) {
            if a.is_empty() || b.is_empty() {
                continue;
            }
            let mut add = |name| {
                let index = names.len();
                match inline_person::encode(name) {
                    Some(key) => match inline_ids.entry(key) {
                        Entry::Occupied(entry) => return usize::try_from(*entry.get()).unwrap(),
                        Entry::Vacant(entry) => {
                            entry.insert(checked_index(index));
                        }
                    },
                    None => match ids.entry(name) {
                        Entry::Occupied(entry) => return usize::try_from(*entry.get()).unwrap(),
                        Entry::Vacant(entry) => {
                            entry.insert(checked_index(index));
                        }
                    },
                }
                names.push(name);
                parents.push(index);
                sizes.push(1usize);
                minima.push(index);
                index
            };
            let ia = add(a);
            let ib = add(b);
            let mut ra = root(&mut parents, ia);
            let mut rb = root(&mut parents, ib);
            if ra != rb {
                if sizes[ra] < sizes[rb] {
                    std::mem::swap(&mut ra, &mut rb);
                }
                parents[rb] = ra;
                sizes[ra] += sizes[rb];
                if names[minima[rb]] < names[minima[ra]] {
                    minima[ra] = minima[rb];
                }
            }
        }
    }
    for index in ids.values_mut().chain(inline_ids.values_mut()) {
        let minimum = minima[root(&mut parents, usize::try_from(*index).unwrap())];
        *index = inline_person::encode(names[minimum]).unwrap_or_else(|| named_person(minimum));
    }
    Identity {
        ids,
        inline_ids,
        names,
    }
}
const NAMED_PERSON: u64 = 0x4000_0000_0000_0000;
const INVALID_PERSON: u64 = 0x5000_0000_0000_0000;
const LOCAL_PERSON: u64 = 0x6000_0000_0000_0000;
const PERSON_TAG: u64 = 0xf000_0000_0000_0000;
const PERSON_PAYLOAD: u64 = 0x0fff_ffff_ffff_ffff;
fn checked_index(index: usize) -> u64 {
    let index = u64::try_from(index).expect("person index exceeds u64 capacity");
    assert!(
        index <= PERSON_PAYLOAD,
        "person index exceeds tagged ID capacity"
    );
    index
}
fn named_person(index: usize) -> u64 {
    NAMED_PERSON | checked_index(index)
}
#[inline]
fn person_partition(person: u64, partitions: usize) -> usize {
    let mut h = person.wrapping_mul(0x9e3779b185ebca87);
    h ^= h >> 32;
    (h as usize) % partitions
}
impl<'a> Identity<'a> {
    #[inline]
    fn lookup(&self, name: &[u8]) -> Option<u64> {
        if let Some(key) = inline_person::encode(name) {
            // An unlinked inline name is already its own globally unique key.
            return Some(self.inline_ids.get(&key).copied().unwrap_or(key));
        }
        self.ids.get(name).copied()
    }
}
struct LocalPeople<'a> {
    names: Vec<&'a [u8]>,
    ids: Map<&'a [u8], u64>,
    worker: u64,
    worker_bits: u32,
}
impl<'a> LocalPeople<'a> {
    fn new(worker: usize, worker_bits: u32) -> Self {
        Self {
            names: Vec::new(),
            ids: Map::default(),
            worker: u64::try_from(worker).unwrap(),
            worker_bits,
        }
    }
    #[inline]
    fn intern(&mut self, name: &'a [u8], identity: &Identity<'a>) -> u64 {
        if let Some(id) = identity.lookup(name) {
            return id;
        }
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let local = checked_index(self.names.len());
        assert!(
            local <= (PERSON_PAYLOAD >> self.worker_bits),
            "worker person dictionary exceeds ID capacity"
        );
        let id = LOCAL_PERSON | (local << self.worker_bits) | self.worker;
        self.names.push(name);
        self.ids.insert(name, id);
        id
    }
}
fn merge_people<'a>(identity: &mut Identity<'a>, workers: Vec<Vec<&'a [u8]>>) -> Vec<Vec<u64>> {
    use std::collections::hash_map::Entry;
    let reserve = workers.iter().map(Vec::len).max().unwrap_or(0);
    let mut unknown = Map::with_capacity_and_hasher(reserve, Default::default());
    let remaps = workers
        .into_iter()
        .map(|names| {
            names
                .into_iter()
                .map(|name| match unknown.entry(name) {
                    Entry::Occupied(entry) => *entry.get(),
                    Entry::Vacant(entry) => {
                        let id = named_person(identity.names.len());
                        identity.names.push(name);
                        entry.insert(id);
                        id
                    }
                })
                .collect()
        })
        .collect();
    identity.ids.reserve(unknown.len());
    for (name, id) in unknown {
        assert!(
            identity.ids.insert(name, id).is_none(),
            "unknown was already linked"
        );
    }
    remaps
}
#[inline]
fn resolve_person(person: u64, remaps: &[Vec<u64>], worker_bits: u32) -> u64 {
    if person & PERSON_TAG != LOCAL_PERSON {
        return person;
    }
    let payload = person & PERSON_PAYLOAD;
    let worker = payload & ((1 << worker_bits) - 1);
    let local = payload >> worker_bits;
    remaps[usize::try_from(worker).unwrap()][usize::try_from(local).unwrap()]
}
struct Catalog<'a> {
    products: Map<&'a [u8], u32>,
    categories: Vec<&'a [u8]>,
    campaigns: Map<&'a [u8], u32>,
    campaign_values: Vec<(&'a [u8], &'a [u8])>,
    organic: u32,
}
fn catalog<'a>(products: &'a [u8], campaigns: &'a [u8]) -> Catalog<'a> {
    let products_rows: Vec<_> = rows::<2>(body(products)).collect();
    let mut categories: Vec<_> = products_rows
        .iter()
        .map(|r| r[1])
        .chain(std::iter::once(&b"unknown"[..]))
        .collect();
    categories.sort_unstable();
    categories.dedup();
    let mut pm = Map::default();
    for [p, c] in products_rows {
        pm.insert(p, categories.binary_search(&c).unwrap() as u32);
    }
    let mut cm = Map::default();
    let mut cv = Vec::new();
    for [id, channel] in rows::<2>(body(campaigns)) {
        cm.insert(id, cv.len() as u32);
        cv.push((id, channel));
    }
    let organic = cv.len() as u32;
    cv.push((b"organic", b"organic"));
    Catalog {
        products: pm,
        categories,
        campaigns: cm,
        campaign_values: cv,
        organic,
    }
}

#[derive(Clone, Copy)]
// Validation is computed while fields are hot, but invalid copies remain as
// tombstones (INVALID_PERSON sentinel) until the earliest ingest is selected globally.
struct Raw {
    id: u64,
    person: u64,
    ingest: i64,
    ts: i64,
    revenue: u32,
    category: u32,
}
#[derive(Clone, Copy)]
struct Event {
    ts: i64,
    person: u64,
    revenue: u32,
    category: u32,
}
#[derive(Clone, Copy)]
struct Click<'a> {
    id: &'a [u8],
    ts: i64,
    person: u64,
    campaign: u32,
}
struct ParsedEvents<'a> {
    raw: Vec<Vec<Vec<Raw>>>,
    unknown: Vec<Vec<&'a [u8]>>,
    fallback_events: Vec<Vec<EventIdName<'a>>>,
    worker_bits: u32,
}
#[cfg(test)]
fn empty_parts<T>(count: usize) -> Vec<Vec<T>> {
    (0..count).map(|_| Vec::new()).collect()
}
fn reserved_parts<T>(count: usize, total_rows: usize, workers: usize) -> Vec<Vec<T>> {
    // A capacity hint changes allocation only. Reallocation handles uneven
    // distribution and arbitrary line lengths without affecting any record.
    let budget = std::env::var("MEM_GB")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(24)
        .clamp(1, 1024)
        .saturating_mul(1024 * 1024 * 1024)
        / 4;
    let rows = total_rows.saturating_mul(11) / 10;
    let per = (rows / workers / count + 1)
        .min(budget / workers / count / std::mem::size_of::<T>().max(1));
    (0..count).map(|_| Vec::with_capacity(per)).collect()
}
fn estimated_rows(tasks: &[&[u8]]) -> usize {
    let bytes: usize = tasks.iter().map(|t| t.len()).sum();
    let Some(sample) = tasks.first() else {
        return 0;
    };
    let sample = &sample[..sample.len().min(65536)];
    let lines = sample.iter().filter(|&&b| b == b'\n').count();
    if sample.is_empty() {
        0
    } else {
        bytes.saturating_mul(lines) / sample.len()
    }
}
fn transpose<T>(workers: Vec<Vec<Vec<T>>>, partitions: usize) -> Vec<Vec<Vec<T>>> {
    let mut result: Vec<Vec<Vec<T>>> = (0..partitions)
        .map(|_| Vec::with_capacity(workers.len()))
        .collect();
    for parts in workers {
        for (i, part) in parts.into_iter().enumerate() {
            result[i].push(part);
        }
    }
    result
}
fn flatten<T>(parts: Vec<Vec<T>>) -> Vec<T> {
    let mut result = Vec::with_capacity(parts.iter().map(Vec::len).sum());
    for mut part in parts {
        result.append(&mut part);
    }
    result
}
fn parse_events<'a>(
    tasks: &[&'a [u8]],
    identity: &Identity<'a>,
    catalog: &Catalog<'a>,
    start: i64,
    end: i64,
    ncpu: usize,
    partitions: usize,
) -> ParsedEvents<'a> {
    let next = AtomicUsize::new(0);
    let worker_bits = ncpu.next_power_of_two().trailing_zeros();
    let workers = thread::scope(|s| {
        let handles: Vec<_> = (0..ncpu)
            .map(|worker| {
                let next = &next;
                s.spawn(move || {
                    let mut result = reserved_parts(partitions, estimated_rows(tasks), ncpu);
                    let mut people = LocalPeople::new(worker, worker_bits);
                    let mut event_ids = LocalEventIds::new(worker, worker_bits);
                    let mut timestamps = TimestampParser::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(bytes) = tasks.get(i) else {
                            break;
                        };
                        for f in rows::<7>(bytes) {
                            if f[0].is_empty() {
                                continue;
                            }
                            let Some(ingest) = timestamps.ingest(f[3]) else {
                                continue;
                            };
                            let id = event_ids.intern(f[0]);
                            let hash = event_ids.hash(id);
                            let (person, ts, revenue, category) = match validate_fields(
                                f,
                                ingest,
                                catalog,
                                start,
                                end,
                                &mut timestamps,
                            ) {
                                Some((name, ts, revenue, category)) => {
                                    (people.intern(name, identity), ts, revenue, category)
                                }
                                None => (INVALID_PERSON, 0, 0, 0),
                            };
                            result[(hash as usize) % partitions].push(Raw {
                                id,
                                person,
                                ingest,
                                ts,
                                revenue,
                                category,
                            });
                        }
                    }
                    (result, people.names, event_ids.names)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    let mut records = Vec::with_capacity(ncpu);
    let mut unknown = Vec::with_capacity(ncpu);
    let mut fallback_events = Vec::with_capacity(ncpu);
    for (rows, people, event_ids) in workers {
        records.push(rows);
        unknown.push(people);
        fallback_events.push(event_ids);
    }
    ParsedEvents {
        raw: transpose(records, partitions),
        unknown,
        fallback_events,
        worker_bits,
    }
}
#[cfg(test)]
#[inline]
fn raw_id(raw: Raw, event_id_remaps: &EventIdRemaps) -> u64 {
    event_id_remaps.canonical(raw.id)
}
#[inline]
fn accepted(raw: Raw, remaps: &[Vec<u64>], worker_bits: u32) -> Option<Event> {
    if raw.person == INVALID_PERSON {
        return None;
    }
    Some(Event {
        person: resolve_person(raw.person, remaps, worker_bits),
        ts: raw.ts,
        revenue: raw.revenue,
        category: raw.category,
    })
}
fn validate_fields<'a>(
    f: [&'a [u8]; 7],
    ingest: i64,
    catalog: &Catalog<'a>,
    start: i64,
    end: i64,
    timestamps: &mut TimestampParser,
) -> Option<(&'a [u8], i64, u32, u32)> {
    if f[1].is_empty() || f[5].is_empty() {
        return None;
    }
    let ts = timestamps.parse(f[2])?;
    if ts < start || ts >= end || ingest - ts > DAY {
        return None;
    }
    let revenue = match f[4] {
        b"view" | b"cart" => 0,
        b"purchase" => {
            let price = digits(f[6])?;
            if !(1..=10_000_000).contains(&price) {
                return None;
            }
            price as u32
        }
        _ => return None,
    };
    let category =
        if revenue == 0 {
            0
        } else {
            catalog.products.get(f[5]).copied().unwrap_or_else(|| {
                catalog.categories.binary_search(&&b"unknown"[..]).unwrap() as u32
            })
        };
    Some((f[1], ts, revenue, category))
}
#[cfg(test)]
fn dedup_events_baseline(
    inputs: Vec<Vec<Vec<Raw>>>,
    event_id_remaps: &EventIdRemaps,
    remaps: &[Vec<u64>],
    worker_bits: u32,
    ncpu: usize,
    partitions: usize,
) -> Vec<Vec<Vec<Event>>> {
    let jobs: Vec<_> = inputs.into_iter().map(|x| Mutex::new(Some(x))).collect();
    let next = AtomicUsize::new(0);
    let workers = thread::scope(|s| {
        let handles: Vec<_> = (0..ncpu)
            .map(|_| {
                s.spawn(|| {
                    let mut output = empty_parts(partitions);
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(i) else {
                            break;
                        };
                        let mut rows = flatten(job.lock().unwrap().take().unwrap());
                        rows.sort_unstable_by_key(|r| event_id_remaps.hash(r.id));
                        let mut pos = 0;
                        while pos < rows.len() {
                            let mut last = pos + 1;
                            while last < rows.len()
                                && event_id_remaps.hash(rows[last].id)
                                    == event_id_remaps.hash(rows[pos].id)
                            {
                                last += 1;
                            }
                            if last == pos + 1 {
                                if let Some(e) = accepted(rows[pos], remaps, worker_bits) {
                                    output[person_partition(e.person, partitions)].push(e);
                                }
                            } else {
                                // Usually one event and a few duplicates. Preserve separate
                                // IDs even if their full 64-bit hashes collide.
                                let mut kept: Vec<Raw> = Vec::with_capacity(1);
                                for &r in &rows[pos..last] {
                                    let id = raw_id(r, event_id_remaps);
                                    if let Some(previous) =
                                        kept.iter_mut().find(|p| raw_id(**p, event_id_remaps) == id)
                                    {
                                        if r.ingest < previous.ingest {
                                            *previous = r;
                                        }
                                    } else {
                                        kept.push(r);
                                    }
                                }
                                for r in kept {
                                    if let Some(e) = accepted(r, remaps, worker_bits) {
                                        output[person_partition(e.person, partitions)].push(e);
                                    }
                                }
                            }
                            pos = last;
                        }
                    }
                    output
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    transpose(workers, partitions)
}

/// Slots pair a 32-bit fingerprint with a checked 32-bit record reference.
/// At most 70% of slots can be occupied even if every row has a distinct ID.
fn dedup_slot_capacity(records: usize) -> Option<usize> {
    if records > u32::MAX as usize {
        return None;
    }
    records
        .checked_mul(10)?
        .checked_add(6)?
        .checked_div(7)?
        .max(1)
        .checked_next_power_of_two()
}

#[inline]
fn emit_dedup_row(row: Raw, remaps: &[Vec<u64>], worker_bits: u32, output: &mut [Vec<Event>]) {
    if let Some(event) = accepted(row, remaps, worker_bits) {
        let partition = person_partition(event.person, output.len());
        output[partition].push(event);
    }
}

/// Fallback for a part that cannot be represented by the compact table.
fn dedup_rows_sorted(
    rows: &mut [Raw],
    event_id_remaps: &EventIdRemaps,
    remaps: &[Vec<u64>],
    worker_bits: u32,
    output: &mut [Vec<Event>],
) {
    rows.sort_unstable_by_key(|row| event_id_remaps.hash(row.id));
    let mut first = 0;
    while first < rows.len() {
        let mut end = first + 1;
        while end < rows.len()
            && event_id_remaps.hash(rows[end].id) == event_id_remaps.hash(rows[first].id)
        {
            end += 1;
        }
        let mut earliest = rows[first];
        let mut collision = false;
        for &row in &rows[first + 1..end] {
            if !event_id_remaps.equal(row.id, earliest.id) {
                collision = true;
                break;
            }
            if row.ingest < earliest.ingest {
                earliest = row;
            }
        }
        if !collision {
            emit_dedup_row(earliest, remaps, worker_bits, output);
        } else {
            let mut kept: Vec<Raw> = Vec::new();
            for &row in &rows[first..end] {
                if let Some(previous) = kept
                    .iter_mut()
                    .find(|old| event_id_remaps.equal(old.id, row.id))
                {
                    if row.ingest < previous.ingest {
                        *previous = row;
                    }
                } else {
                    kept.push(row);
                }
            }
            for row in kept {
                emit_dedup_row(row, remaps, worker_bits, output);
            }
        }
        first = end;
    }
}

/// Returns false before emitting anything if compact indices cannot represent
/// this part. Raw records stay immutable, including invalid-copy tombstones.
#[cfg(test)]
fn dedup_rows_slots<const LINEAR_OUTPUT: bool>(
    rows: &[Raw],
    event_id_remaps: &EventIdRemaps,
    remaps: &[Vec<u64>],
    worker_bits: u32,
    output: &mut [Vec<Event>],
) -> bool {
    let Some(capacity) = dedup_slot_capacity(rows.len()) else {
        return false;
    };
    let mask = capacity - 1;
    let mut slots = vec![0u64; capacity];
    let mut winners = if LINEAR_OUTPUT {
        vec![0u8; rows.len()]
    } else {
        Vec::new()
    };
    for (index, row) in rows.iter().enumerate() {
        let hash = event_id_remaps.hash(row.id);
        let fingerprint = hash as u32;
        let encoded = (u64::from(fingerprint) << 32) | (index as u64 + 1);
        // Low hash bits already selected the outer part. Use the high half
        // here to avoid clustering from those fixed low bits.
        let mut position = (hash >> 32) as usize & mask;
        loop {
            let slot = slots[position];
            if slot == 0 {
                slots[position] = encoded;
                if LINEAR_OUTPUT {
                    winners[index] = 1;
                }
                break;
            }
            // The fingerprint rejects almost every occupied probe without a
            // random Raw load. It never decides identity on its own.
            if (slot >> 32) as u32 == fingerprint {
                let previous_index = (slot as u32 - 1) as usize;
                let previous = &rows[previous_index];
                if event_id_remaps.equal(previous.id, row.id) {
                    if row.ingest < previous.ingest {
                        slots[position] = encoded;
                        if LINEAR_OUTPUT {
                            winners[previous_index] = 0;
                            winners[index] = 1;
                        }
                    }
                    break;
                }
            }
            position = (position + 1) & mask;
        }
    }
    if LINEAR_OUTPUT {
        // A byte per original record replaces random survivor gathers with
        // one sequential Raw scan. Only duplicate replacements write backwards.
        for (row, &winner) in rows.iter().zip(&winners) {
            if winner != 0 {
                emit_dedup_row(*row, remaps, worker_bits, output);
            }
        }
    } else {
        for slot in slots {
            if slot != 0 {
                emit_dedup_row(
                    rows[(slot as u32 - 1) as usize],
                    remaps,
                    worker_bits,
                    output,
                );
            }
        }
    }
    true
}

/// A checked 32-bit slot reference. The low bits hold local row index+1;
/// the high bits hold a chunk index. Zero therefore never denotes a row.
#[derive(Clone, Copy)]
struct ChunkRefLayout {
    row_bits: u32,
    row_mask: u64,
}
impl ChunkRefLayout {
    fn for_lengths(lengths: impl Iterator<Item = usize>) -> Option<(Self, usize)> {
        let mut chunks = 0usize;
        let mut rows = 0usize;
        let mut longest = 0usize;
        for len in lengths {
            chunks = chunks.checked_add(1)?;
            rows = rows.checked_add(len)?;
            longest = longest.max(len);
        }
        if rows > u32::MAX as usize {
            return None;
        }
        // ceil(log2(longest+1)), without overflowing at usize::MAX.
        let row_bits = (usize::BITS - longest.leading_zeros()).max(1);
        if row_bits > 32 || chunks as u64 > (1u64 << (32 - row_bits)) {
            return None;
        }
        Some((
            Self {
                row_bits,
                row_mask: (1u64 << row_bits) - 1,
            },
            rows,
        ))
    }
    #[inline]
    fn split(self, reference: u32) -> (usize, usize) {
        let reference = reference as u64;
        (
            (reference >> self.row_bits) as usize,
            ((reference & self.row_mask) - 1) as usize,
        )
    }
}

/// No Raw records move on this path. Only fingerprint matches read an old Raw
/// through its chunk reference, and selected records are emitted sequentially.
fn dedup_chunks(
    chunks: &[Vec<Raw>],
    event_id_remaps: &EventIdRemaps,
    remaps: &[Vec<u64>],
    worker_bits: u32,
    output: &mut [Vec<Event>],
) -> bool {
    let Some((layout, count)) = ChunkRefLayout::for_lengths(chunks.iter().map(Vec::len)) else {
        return false;
    };
    let Some(capacity) = dedup_slot_capacity(count) else {
        return false;
    };
    let mask = capacity - 1;
    let mut slots = vec![0u64; capacity];
    let mut winners: Vec<Vec<u8>> = chunks.iter().map(|rows| vec![0u8; rows.len()]).collect();
    for (chunk_index, rows) in chunks.iter().enumerate() {
        let chunk_prefix = (chunk_index as u64) << layout.row_bits;
        for (index, row) in rows.iter().enumerate() {
            let hash = event_id_remaps.hash(row.id);
            let fingerprint = hash as u32;
            let reference = chunk_prefix | (index as u64 + 1);
            let encoded = (u64::from(fingerprint) << 32) | reference;
            let mut position = (hash >> 32) as usize & mask;
            loop {
                let slot = slots[position];
                if slot == 0 {
                    slots[position] = encoded;
                    winners[chunk_index][index] = 1;
                    break;
                }
                if (slot >> 32) as u32 == fingerprint {
                    let (old_chunk, old_index) = layout.split(slot as u32);
                    let previous = &chunks[old_chunk][old_index];
                    if event_id_remaps.equal(previous.id, row.id) {
                        if row.ingest < previous.ingest {
                            slots[position] = encoded;
                            winners[old_chunk][old_index] = 0;
                            winners[chunk_index][index] = 1;
                        }
                        break;
                    }
                }
                position = (position + 1) & mask;
            }
        }
    }
    for (rows, flags) in chunks.iter().zip(&winners) {
        for (&row, &winner) in rows.iter().zip(flags) {
            if winner != 0 {
                emit_dedup_row(row, remaps, worker_bits, output);
            }
        }
    }
    true
}

#[cfg(test)]
fn dedup_chunks_slot_reference<const LINEAR_OUTPUT: bool>(
    chunks: &[Vec<Raw>],
    event_id_remaps: &EventIdRemaps,
    remaps: &[Vec<u64>],
    worker_bits: u32,
    output: &mut [Vec<Event>],
) -> bool {
    let Some((layout, count)) = ChunkRefLayout::for_lengths(chunks.iter().map(Vec::len)) else {
        return false;
    };
    let Some(capacity) = dedup_slot_capacity(count) else {
        return false;
    };
    let mask = capacity - 1;
    let mut slots = vec![0u64; capacity];
    let mut winners: Vec<Vec<u8>> = if LINEAR_OUTPUT {
        chunks.iter().map(|rows| vec![0u8; rows.len()]).collect()
    } else {
        Vec::new()
    };
    for (chunk_index, rows) in chunks.iter().enumerate() {
        let chunk_prefix = (chunk_index as u64) << layout.row_bits;
        for (index, row) in rows.iter().enumerate() {
            let hash = event_id_remaps.hash(row.id);
            let fingerprint = hash as u32;
            let reference = chunk_prefix | (index as u64 + 1);
            let encoded = (u64::from(fingerprint) << 32) | reference;
            let mut position = (hash >> 32) as usize & mask;
            loop {
                let slot = slots[position];
                if slot == 0 {
                    slots[position] = encoded;
                    if LINEAR_OUTPUT {
                        winners[chunk_index][index] = 1;
                    }
                    break;
                }
                if (slot >> 32) as u32 == fingerprint {
                    let (old_chunk, old_index) = layout.split(slot as u32);
                    let previous = &chunks[old_chunk][old_index];
                    if event_id_remaps.equal(previous.id, row.id) {
                        if row.ingest < previous.ingest {
                            slots[position] = encoded;
                            if LINEAR_OUTPUT {
                                winners[old_chunk][old_index] = 0;
                                winners[chunk_index][index] = 1;
                            }
                        }
                        break;
                    }
                }
                position = (position + 1) & mask;
            }
        }
    }
    if LINEAR_OUTPUT {
        for (rows, flags) in chunks.iter().zip(&winners) {
            for (&row, &winner) in rows.iter().zip(flags) {
                if winner != 0 {
                    emit_dedup_row(row, remaps, worker_bits, output);
                }
            }
        }
    } else {
        for slot in slots {
            if slot != 0 {
                let (chunk, index) = layout.split(slot as u32);
                emit_dedup_row(chunks[chunk][index], remaps, worker_bits, output);
            }
        }
    }
    true
}

#[cfg(test)]
fn dedup_chunks_slots<const LINEAR_OUTPUT: bool>(
    chunks: &[Vec<Raw>],
    event_id_remaps: &EventIdRemaps,
    remaps: &[Vec<u64>],
    worker_bits: u32,
    output: &mut [Vec<Event>],
) -> bool {
    if LINEAR_OUTPUT {
        dedup_chunks(chunks, event_id_remaps, remaps, worker_bits, output)
    } else {
        dedup_chunks_slot_reference::<false>(chunks, event_id_remaps, remaps, worker_bits, output)
    }
}

fn dedup_events(
    inputs: Vec<Vec<Vec<Raw>>>,
    event_id_remaps: &EventIdRemaps,
    remaps: &[Vec<u64>],
    worker_bits: u32,
    ncpu: usize,
    partitions: usize,
) -> Vec<Vec<Vec<Event>>> {
    let input_rows: usize = inputs
        .iter()
        .flat_map(|parts| parts.iter())
        .map(Vec::len)
        .sum();
    let jobs: Vec<_> = inputs
        .into_iter()
        .map(|part| Mutex::new(Some(part)))
        .collect();
    let next = AtomicUsize::new(0);
    let workers = thread::scope(|scope| {
        let handles: Vec<_> = (0..ncpu)
            .map(|_| {
                scope.spawn(|| {
                    let mut output = reserved_parts(partitions, input_rows, ncpu);
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(index) else {
                            break;
                        };
                        let chunks = job.lock().unwrap().take().unwrap();
                        if !dedup_chunks(&chunks, event_id_remaps, remaps, worker_bits, &mut output)
                        {
                            // Capacity checks happen before emission. Sorting therefore
                            // safely handles parts that exceed the compact reference.
                            let mut rows = flatten(chunks);
                            dedup_rows_sorted(
                                &mut rows,
                                event_id_remaps,
                                remaps,
                                worker_bits,
                                &mut output,
                            );
                        }
                    }
                    output
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    transpose(workers, partitions)
}

fn parse_clicks<'a>(
    tasks: &[&'a [u8]],
    identity: &Identity<'a>,
    catalog: &Catalog<'a>,
    start: i64,
    end: i64,
    ncpu: usize,
    partitions: usize,
) -> Vec<Vec<Vec<Click<'a>>>> {
    let next = AtomicUsize::new(0);
    let workers = thread::scope(|s| {
        let handles: Vec<_> = (0..ncpu)
            .map(|_| {
                s.spawn(|| {
                    let mut output = reserved_parts(partitions, estimated_rows(tasks), ncpu);
                    let mut timestamps = TimestampParser::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(bytes) = tasks.get(i) else {
                            break;
                        };
                        for [id, user, camp, time] in rows::<4>(bytes) {
                            if id.is_empty() || user.is_empty() {
                                continue;
                            }
                            let Some(&campaign) = catalog.campaigns.get(camp) else {
                                continue;
                            };
                            let Some(ts) = timestamps.parse(time) else {
                                continue;
                            };
                            if ts < start - WEEK || ts >= end {
                                continue;
                            }
                            // An unlinked ID absent from all validated event
                            // candidates cannot own a surviving session. Linked
                            // aliases are all present in the complete dictionary.
                            let Some(person) = identity.lookup(user) else {
                                continue;
                            };
                            output[person_partition(person, partitions)].push(Click {
                                person,
                                id,
                                ts,
                                campaign,
                            });
                        }
                    }
                    output
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    transpose(workers, partitions)
}

fn io_context(error: io::Error, path: &Path) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}
fn make_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path).map_err(|error| io_context(error, path))
}
#[derive(Default, Clone, Copy)]
struct CampaignAgg {
    sessions: i64,
    purchasing: i64,
    revenue: i64,
    assisted: i64,
}
type CampaignMap = Map<(u32, i32), CampaignAgg>;
enum CampaignTotals {
    Dense {
        base: i32,
        campaigns: usize,
        rows: Vec<CampaignAgg>,
    },
    Sparse(CampaignMap),
}
impl CampaignTotals {
    fn new(base: i32, days: usize, campaigns: usize) -> Self {
        match days.checked_mul(campaigns) {
            Some(cells) if cells <= 262_144 => Self::Dense {
                base,
                campaigns,
                rows: vec![CampaignAgg::default(); cells],
            },
            _ => Self::Sparse(CampaignMap::default()),
        }
    }
    #[inline]
    fn at(&mut self, campaign: u32, day: i32) -> &mut CampaignAgg {
        match self {
            Self::Dense {
                base,
                campaigns,
                rows,
            } => &mut rows[(day - *base) as usize * *campaigns + campaign as usize],
            Self::Sparse(rows) => rows.entry((campaign, day)).or_default(),
        }
    }
    fn into_map(self) -> CampaignMap {
        match self {
            Self::Sparse(rows) => rows,
            Self::Dense {
                base,
                campaigns,
                rows,
            } => {
                let mut map = CampaignMap::default();
                for (i, row) in rows.into_iter().enumerate() {
                    if row.sessions != 0 || row.assisted != 0 {
                        map.insert(((i % campaigns) as u32, base + (i / campaigns) as i32), row);
                    }
                }
                map
            }
        }
    }
}
fn write_campaigns(path: PathBuf, rows: CampaignMap, catalog: &Catalog<'_>) -> io::Result<usize> {
    let count = rows.len();
    let mut writer = CampaignWriter::new(path)?;
    for ((c, d), r) in rows {
        let (name, channel) = catalog.campaign_values[c as usize];
        writer.push(
            name,
            channel,
            d,
            r.sessions,
            r.purchasing,
            r.revenue,
            r.assisted,
        )?;
    }
    writer.finish()?;
    Ok(count)
}

#[derive(Default)]
struct UserAgg {
    day: i32,
    sessions: i64,
    events: i64,
    revenue: i64,
}
fn flush_user(
    writer: &mut UserWriter,
    person: &[u8],
    agg: &UserAgg,
    category_sums: &[i64],
    catalog: &Catalog<'_>,
) -> io::Result<()> {
    if agg.sessions == 0 {
        return Ok(());
    }
    let mut best = None;
    let mut best_revenue = 0;
    for (i, &revenue) in category_sums.iter().enumerate() {
        if revenue > best_revenue {
            best = Some(catalog.categories[i]);
            best_revenue = revenue;
        }
    }
    writer.push(person, agg.day, agg.sessions, agg.events, agg.revenue, best)
}
fn sort_events(events: &mut [Event]) {
    events.sort_unstable_by(|a, b| a.person.cmp(&b.person).then(a.ts.cmp(&b.ts)));
}
fn sort_clicks(clicks: &mut [Click<'_>]) {
    clicks.sort_unstable_by(|a, b| {
        a.person
            .cmp(&b.person)
            .then(a.ts.cmp(&b.ts))
            .then_with(|| b.id.cmp(a.id))
    });
}
fn process_partition(
    mut events: Vec<Event>,
    mut clicks: Vec<Click<'_>>,
    catalog: &Catalog<'_>,
    names: &[&[u8]],
    out: PathBuf,
    day_min: i32,
    day_count: usize,
) -> io::Result<(CampaignMap, u64)> {
    sort_events(&mut events);
    sort_clicks(&mut clicks);
    // All copies have identical values. Sorting puts them together after their
    // unquoted values and canonical people have been compared exactly.
    clicks.dedup_by(|a, b| a.person == b.person && a.ts == b.ts && a.id == b.id);
    let mut writer = UserWriter::new(out)?;
    let mut campaign_rows = CampaignTotals::new(day_min, day_count, catalog.campaign_values.len());
    let mut ev = 0;
    let mut ck = 0;
    let nc = catalog.campaign_values.len();
    let mut active_counts = vec![0u32; nc];
    let mut positions = vec![usize::MAX; nc];
    let mut active: Vec<u32> = Vec::new();
    let mut category_sums = vec![0i64; catalog.categories.len()];
    while ev < events.len() {
        let person = events[ev].person;
        let mut decoded = [0u8; 9];
        let name = if person & PERSON_TAG == NAMED_PERSON {
            names[usize::try_from(person & PERSON_PAYLOAD).unwrap()]
        } else {
            inline_person::decode(person, &mut decoded)
        };
        let mut ev_end = ev + 1;
        while ev_end < events.len() && events[ev_end].person == person {
            ev_end += 1;
        }
        while ck < clicks.len() && clicks[ck].person < person {
            ck += 1;
        }
        let ck_start = ck;
        while ck < clicks.len() && clicks[ck].person == person {
            ck += 1;
        }
        let person_clicks = &clicks[ck_start..ck];
        let mut left = 0;
        let mut right = 0;
        let mut agg = UserAgg::default();
        category_sums.fill(0);
        while ev < ev_end {
            let start = events[ev].ts;
            let day = start.div_euclid(DAY) as i32;
            if agg.sessions > 0 && agg.day != day {
                flush_user(&mut writer, name, &agg, &category_sums, catalog)?;
                agg = UserAgg::default();
                category_sums.fill(0);
            }
            agg.day = day;
            agg.sessions += 1;
            let first = ev;
            ev += 1;
            while ev < ev_end && events[ev].ts - events[ev - 1].ts <= GAP {
                ev += 1;
            }
            let mut session_revenue = 0i64;
            for e in &events[first..ev] {
                session_revenue += e.revenue as i64;
                if e.revenue > 0 {
                    category_sums[e.category as usize] += e.revenue as i64;
                }
            }
            agg.events += (ev - first) as i64;
            agg.revenue += session_revenue;
            while right < person_clicks.len() && person_clicks[right].ts <= start {
                let c = person_clicks[right].campaign as usize;
                if active_counts[c] == 0 {
                    positions[c] = active.len();
                    active.push(c as u32);
                }
                active_counts[c] += 1;
                right += 1;
            }
            while left < right && person_clicks[left].ts < start - WEEK {
                let c = person_clicks[left].campaign as usize;
                active_counts[c] -= 1;
                if active_counts[c] == 0 {
                    let pos = positions[c];
                    let last = active.pop().unwrap();
                    if pos < active.len() {
                        active[pos] = last;
                        positions[last as usize] = pos;
                    }
                    positions[c] = usize::MAX;
                }
                left += 1;
            }
            let credit = if right > left {
                person_clicks[right - 1].campaign
            } else {
                catalog.organic
            };
            let row = campaign_rows.at(credit, day);
            row.sessions += 1;
            row.revenue += session_revenue;
            row.purchasing += i64::from(session_revenue > 0);
            for &campaign in &active {
                campaign_rows.at(campaign, day).assisted += 1;
            }
        }
        flush_user(&mut writer, name, &agg, &category_sums, catalog)?;
        for &campaign in &active {
            active_counts[campaign as usize] = 0;
            positions[campaign as usize] = usize::MAX;
        }
        active.clear();
    }
    Ok((campaign_rows.into_map(), writer.finish()?))
}

fn run() -> io::Result<()> {
    let began = Instant::now();
    let data = PathBuf::from(std::env::var_os("DATA_DIR").unwrap_or_else(|| "/data".into()));
    let out = PathBuf::from(std::env::var_os("OUTPUT_DIR").unwrap_or_else(|| "/output".into()));
    for table in ["user_daily", "campaign_daily"] {
        let folder = out.join(table);
        if folder.is_dir() {
            for entry in fs::read_dir(&folder)? {
                if entry?
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "parquet")
                {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "Use a fresh OUTPUT_DIR: {} contains Parquet files",
                            folder.display()
                        ),
                    ));
                }
            }
        }
    }
    make_dir(&out)?;
    // Create shared parents before parallel workers create their disjoint parts.
    make_dir(&out.join("user_daily"))?;
    make_dir(&out.join("campaign_daily"))?;
    let ncpu = std::env::var("NCPU")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(8)
        .clamp(1, 64);
    let manifest = fs::read_to_string(data.join("manifest.json"))?;
    let start = json_time(&manifest, "window_start")?;
    let end = json_time(&manifest, "window_end")?;
    if start >= end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty/reversed time window",
        ));
    }
    let link_data = load_files(&csv_paths(&data.join("links"))?, ncpu)?;
    let product_data = fs::read(data.join("products.csv"))?;
    let campaign_data = fs::read(data.join("campaigns.csv"))?;
    let mut identity = identities(&link_data);
    let catalog = catalog(&product_data, &campaign_data);
    eprintln!(
        "identities: {} IDs, {:.3}s",
        identity.names.len(),
        began.elapsed().as_secs_f64()
    );
    let event_data = load_files(&csv_paths(&data.join("events"))?, ncpu)?;
    // Tiny fixtures need one part, not dozens of empty files. Full datasets
    // retain sixteen independent partitions per CPU for balanced work queues.
    let event_bytes: usize = event_data.iter().map(|f| f.len()).sum();
    let partitions = (event_bytes / CHUNK).clamp(1, ncpu * 16);
    let event_tasks = chunks(&event_data);
    let parsed = parse_events(
        &event_tasks,
        &identity,
        &catalog,
        start,
        end,
        ncpu,
        partitions,
    );
    let raw_count: usize = parsed.raw.iter().flat_map(|v| v.iter()).map(Vec::len).sum();
    eprintln!(
        "parsed events: {raw_count}, {:.3}s",
        began.elapsed().as_secs_f64()
    );
    let unknown_count: usize = parsed.unknown.iter().map(Vec::len).sum();
    let before_people = identity.names.len();
    let merge_start = Instant::now();
    let remaps = merge_people(&mut identity, parsed.unknown);
    eprintln!("interned people: local_unknown={unknown_count}, unique_unknown={}, merge={:.3}s, total={:.3}s",
        identity.names.len() - before_people, merge_start.elapsed().as_secs_f64(), began.elapsed().as_secs_f64());
    let fallback_count: usize = parsed.fallback_events.iter().map(Vec::len).sum();
    let event_id_remaps = EventIdRemaps::merge(parsed.fallback_events, parsed.worker_bits);
    eprintln!(
        "event ID remaps: {fallback_count}, {:.3}s",
        began.elapsed().as_secs_f64()
    );
    let events = dedup_events(
        parsed.raw,
        &event_id_remaps,
        &remaps,
        parsed.worker_bits,
        ncpu,
        partitions,
    );
    let event_count: usize = events.iter().flat_map(|v| v.iter()).map(Vec::len).sum();
    eprintln!(
        "validated events: {event_count}, {:.3}s",
        began.elapsed().as_secs_f64()
    );
    let click_data = load_files(&csv_paths(&data.join("clicks"))?, ncpu)?;
    let click_tasks = chunks(&click_data);
    let clicks = parse_clicks(
        &click_tasks,
        &identity,
        &catalog,
        start,
        end,
        ncpu,
        partitions,
    );
    let click_count: usize = clicks.iter().flat_map(|v| v.iter()).map(Vec::len).sum();
    eprintln!(
        "validated clicks: {click_count}, {:.3}s",
        began.elapsed().as_secs_f64()
    );
    let jobs: Vec<_> = events
        .into_iter()
        .zip(clicks)
        .enumerate()
        .map(|(i, (e, c))| Mutex::new(Some((i, e, c))))
        .collect();
    let next = AtomicUsize::new(0);
    let results = thread::scope(|s| {
        let handles: Vec<_> = (0..ncpu)
            .map(|_| {
                s.spawn(|| -> io::Result<(CampaignMap, u64)> {
                    let mut combined = CampaignMap::default();
                    let mut total_users = 0;
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(i) else {
                            break;
                        };
                        let (part, e, c) = job.lock().unwrap().take().unwrap();
                        let (rows, count) = process_partition(
                            flatten(e),
                            flatten(c),
                            &catalog,
                            &identity.names,
                            out.join("user_daily").join(format!("part-{part:03}")),
                            start.div_euclid(DAY) as i32,
                            ((end - 1).div_euclid(DAY) - start.div_euclid(DAY) + 1) as usize,
                        )?;
                        total_users += count;
                        for (key, value) in rows {
                            let r = combined.entry(key).or_default();
                            r.sessions += value.sessions;
                            r.purchasing += value.purchasing;
                            r.revenue += value.revenue;
                            r.assisted += value.assisted;
                        }
                    }
                    Ok((combined, total_users))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<io::Result<Vec<_>>>()
    })?;
    let mut combined = CampaignMap::default();
    let mut total_users = 0;
    for (rows, count) in results {
        total_users += count;
        for (key, value) in rows {
            let r = combined.entry(key).or_default();
            r.sessions += value.sessions;
            r.purchasing += value.purchasing;
            r.revenue += value.revenue;
            r.assisted += value.assisted;
        }
    }
    let campaign_count = write_campaigns(
        out.join("campaign_daily").join("part-000"),
        combined,
        &catalog,
    )?;
    eprintln!(
        "wrote user_daily={total_users}, campaign_daily={campaign_count}; native total {:.3}s",
        began.elapsed().as_secs_f64()
    );
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("shopping-sessions: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn digits_match_checked_reference_at_numeric_boundaries() {
        fn reference(bytes: &[u8]) -> Option<i64> {
            if bytes.is_empty() {
                return None;
            }
            let mut n = 0i64;
            for &b in bytes {
                if !b.is_ascii_digit() {
                    return None;
                }
                n = n.checked_mul(10)?.checked_add((b - b'0') as i64)?;
            }
            Some(n)
        }
        for value in [
            "",
            "0",
            "000",
            "9223372036854775807",
            "9223372036854775808",
            "18446744073709551615",
            "000000000000000000000000001791071890000",
            "000000000000000000000000000000000x",
        ] {
            assert_eq!(digits(value.as_bytes()), reference(value.as_bytes()));
        }
        for pos in 0..19 {
            for byte in 0..=255u8 {
                let mut value = *b"1791071890000123456";
                value[pos] = byte;
                assert_eq!(digits(&value), reference(&value), "{value:?}");
            }
        }
    }
    #[test]
    fn record_layout_is_compact() {
        assert_eq!(std::mem::size_of::<Raw>(), 40);
        assert_eq!(std::mem::size_of::<Event>(), 24);
        assert_eq!(std::mem::size_of::<Click>(), 40);
    }
    #[test]
    fn unknown_people_merge_across_workers_and_preserve_exact_bytes() {
        let mut identity = Identity {
            ids: Map::default(),
            inline_ids: Map::default(),
            names: vec![b"unused minimum"],
        };
        identity
            .inline_ids
            .insert(inline_person::encode(b"alias").unwrap(), named_person(0));
        let mut a = LocalPeople::new(0, 2);
        let mut b = LocalPeople::new(3, 2);
        assert_eq!(a.intern(b"alias", &identity), named_person(0));
        // Long/UTF-8 names take the exact byte-keyed fallback across workers.
        let shared = " user-Ω\0\r".as_bytes();
        let distinct = " User-Ω\0\r".as_bytes();
        let ak = a.intern(shared, &identity);
        let bk = b.intern(shared, &identity);
        let other = b.intern(distinct, &identity);
        let direct = a.intern(b"\0\r001", &identity);
        assert_eq!(direct, b.intern(b"\0\r001", &identity));
        let remaps = merge_people(&mut identity, vec![a.names, vec![], vec![], b.names]);
        assert_eq!(
            resolve_person(ak, &remaps, 2),
            resolve_person(bk, &remaps, 2)
        );
        assert_ne!(
            resolve_person(ak, &remaps, 2),
            resolve_person(other, &remaps, 2)
        );
        assert_eq!(resolve_person(direct, &remaps, 2), direct);
        assert_eq!(identity.names.len(), 3);
        assert_eq!(identity.lookup(b"alias"), Some(named_person(0)));
    }
    #[test]
    fn inline_keys_and_fallback_tags_never_alias() {
        for len in 1..=9 {
            for value in 0..128u8 {
                let name = vec![value; len];
                let key = inline_person::encode(&name).unwrap();
                assert_ne!(key, INVALID_PERSON);
                assert_ne!(key & PERSON_TAG, NAMED_PERSON);
                assert_ne!(key & PERSON_TAG, LOCAL_PERSON);
                let mut buffer = [0; 9];
                assert_eq!(inline_person::decode(key, &mut buffer), name);
            }
        }
        assert_eq!(inline_person::encode(&[127; 9]), Some(u64::MAX));
        assert_eq!(resolve_person(u64::MAX, &[], 0), u64::MAX);
    }
    #[test]
    fn event_dedup_preserves_invalid_earliest() {
        let later = Raw {
            id: inline_person::encode(b"x").unwrap(),
            person: 1,
            ingest: 200,
            ts: 10,
            revenue: 1,
            category: 0,
        };
        let invalid = Raw {
            ingest: 100,
            person: INVALID_PERSON,
            ..later
        };
        let other = Raw {
            id: inline_person::encode(b"y").unwrap(),
            person: 2,
            ingest: 150,
            ts: 20,
            revenue: 7,
            category: 0,
        };
        let other_later = Raw {
            ingest: 180,
            revenue: 9,
            ..other
        };
        let output = dedup_events(
            vec![vec![vec![later, other_later, invalid, other]]],
            &EventIdRemaps::default(),
            &[],
            0,
            1,
            1,
        );
        let kept: Vec<_> = output.into_iter().flatten().flatten().collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].person, 2);
        assert_eq!(kept[0].revenue, 7);
    }
    #[test]
    fn dense_sorting_preserves_timestamps_and_click_id_tie_order() {
        let mut events = vec![
            Event {
                person: 2,
                ts: 1,
                revenue: 0,
                category: 0,
            },
            Event {
                person: 1,
                ts: 4,
                revenue: 0,
                category: 0,
            },
            Event {
                person: 1,
                ts: 0,
                revenue: 0,
                category: 0,
            },
        ];
        sort_events(&mut events);
        assert_eq!(
            events.iter().map(|e| (e.person, e.ts)).collect::<Vec<_>>(),
            vec![(1, 0), (1, 4), (2, 1)]
        );
        let mut clicks = vec![
            Click {
                person: 2,
                ts: 1,
                id: b"a",
                campaign: 0,
            },
            Click {
                person: 1,
                ts: 1,
                id: b"A",
                campaign: 0,
            },
            Click {
                person: 1,
                ts: 1,
                id: b"\0",
                campaign: 0,
            },
            Click {
                person: 1,
                ts: 2,
                id: b"later",
                campaign: 0,
            },
        ];
        sort_clicks(&mut clicks);
        assert_eq!(clicks[1].id, b"\0");
        assert_eq!(clicks[2].id, b"later");
        sort_events(&mut []);
        sort_clicks(&mut []);
    }
    #[test]
    fn strict_csv() {
        assert!(fields::<2>(b"a,\"b\"").is_some());
        assert!(fields::<2>(b"a,b\r").is_some());
        assert!(fields::<2>(b"a,\"b\"\r").is_none());
        assert!(fields::<2>(b"a,b,c").is_none());
        assert_eq!(fields::<2>(b"\"\",\" u\"").unwrap(), [&b""[..], &b" u"[..]]);
    }
    #[test]
    fn dates() {
        assert_eq!(timestamp(b"1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            timestamp(b"2028-02-29T00:00:00Z"),
            timestamp(b"2028-02-28T23:00:00-01:00")
        );
        for b in [
            b"2026-02-29T00:00:00Z".as_slice(),
            b"2028-02-30T00:00:00Z",
            b"2026-01-01T24:00:00Z",
            b"2026-01-01T00:00:00+24:00",
            b"2026-01-01T00:00:00Z\r",
        ] {
            assert_eq!(timestamp(b), None);
        }
    }
    #[test]
    fn long_numbers() {
        assert_eq!(digits(b"000000000000000000000000000000250"), Some(250));
        assert_eq!(timestamp(b"0000000000000000000000000000000000000"), Some(0));
        assert_eq!(timestamp(b"9999999999999999999999999999999"), None);
        assert_eq!(digits("\u{0662}\u{0665}\u{0660}".as_bytes()), None);
    }
}

#[cfg(test)]
mod combined_id_tests {
    use super::*;
    #[test]
    fn both_remap_domains_and_invalid_minimum_survive_hash_collisions() {
        let mut identity = Identity {
            ids: Map::default(),
            inline_ids: Map::default(),
            names: Vec::new(),
        };
        let mut people_a = LocalPeople::new(0, 2);
        let mut people_b = LocalPeople::new(3, 2);
        let pa = people_a.intern("long-person-Ω".as_bytes(), &identity);
        let pb = people_b.intern("long-person-Ω".as_bytes(), &identity);
        let other = people_b.intern("long-person-ω".as_bytes(), &identity);
        let person_remaps = merge_people(
            &mut identity,
            vec![people_a.names, vec![], vec![], people_b.names],
        );
        let mut ids_a = LocalEventIds::new(0, 2);
        let mut ids_b = LocalEventIds::new(3, 2);
        let ea = ids_a.intern(b"shared-long-event");
        let eb = ids_b.intern(b"shared-long-event");
        let da = ids_a.intern(b"invalid-long-event");
        let db = ids_b.intern(b"invalid-long-event");
        let distinct = ids_b.intern(b"Shared-long-event");
        let mut event_remaps =
            EventIdRemaps::merge(vec![ids_a.names, vec![], vec![], ids_b.names], 2);
        // Force full-hash collisions between all fallback IDs and nine DEL bytes.
        event_remaps.force_fallback_hashes(mix64(u64::MAX));
        let raw = |id, person, ingest| Raw {
            id,
            person,
            ingest,
            ts: ingest,
            revenue: 1,
            category: 0,
        };
        let input = vec![
            raw(ea, pa, 50),
            raw(da, pb, 40),
            raw(distinct, other, 30),
            raw(eb, pb, 10),
            raw(db, INVALID_PERSON, 1),
            raw(u64::MAX, u64::MAX, 20),
        ];
        assert!(input
            .iter()
            .all(|row| event_remaps.hash(row.id) == mix64(u64::MAX)));
        let mut expected = vec![(named_person(0), 10), (named_person(1), 30), (u64::MAX, 20)];
        expected.sort_unstable();
        for linear in [false, true] {
            let mut output = empty_parts(4);
            assert!(if linear {
                dedup_rows_slots::<true>(&input, &event_remaps, &person_remaps, 2, &mut output)
            } else {
                dedup_rows_slots::<false>(&input, &event_remaps, &person_remaps, 2, &mut output)
            });
            let mut actual = Vec::new();
            for (partition, events) in output.iter().enumerate() {
                for event in events {
                    assert_eq!(partition, person_partition(event.person, 4));
                    actual.push((event.person, event.ts));
                }
            }
            actual.sort_unstable();
            assert_eq!(actual, expected);
        }
        let mut fallback = input.clone();
        let mut output = vec![Vec::new()];
        dedup_rows_sorted(&mut fallback, &event_remaps, &person_remaps, 2, &mut output);
        let mut actual: Vec<_> = output[0].iter().map(|e| (e.person, e.ts)).collect();
        actual.sort_unstable();
        assert_eq!(actual, expected);
        let output =
            dedup_events_baseline(vec![vec![input]], &event_remaps, &person_remaps, 2, 1, 1);
        let mut actual: Vec<_> = output
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| (e.person, e.ts))
            .collect();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }
}

#[cfg(test)]
mod chunk_dedup_tests {
    use super::*;
    #[test]
    fn reference_layout_boundaries_are_checked_without_large_allocations() {
        for lengths in [
            vec![],
            vec![0],
            vec![0; 8],
            vec![1; 8],
            vec![3, 100, 0, 19],
            vec![131071; 8],
        ] {
            let (layout, total) = ChunkRefLayout::for_lengths(lengths.iter().copied()).unwrap();
            assert_eq!(total, lengths.iter().sum::<usize>());
            for (chunk, &len) in lengths.iter().enumerate() {
                if len > 0 {
                    for index in [0, len - 1] {
                        let reference = ((chunk as u64) << layout.row_bits) | (index as u64 + 1);
                        assert!(reference > 0 && reference <= u32::MAX as u64);
                        assert_eq!(layout.split(reference as u32), (chunk, index));
                    }
                }
            }
        }
        let (layout, total) = ChunkRefLayout::for_lengths([u32::MAX as usize].into_iter()).unwrap();
        assert_eq!(total, u32::MAX as usize);
        assert_eq!(layout.split(u32::MAX), (0, u32::MAX as usize - 1));
        assert!(ChunkRefLayout::for_lengths([u32::MAX as usize, 1].into_iter()).is_none());
        assert!(ChunkRefLayout::for_lengths([1usize << 30, 0, 0, 0, 0].into_iter()).is_none());
        assert!(ChunkRefLayout::for_lengths([usize::MAX, 1].into_iter()).is_none());
    }
    #[test]
    fn chunk_references_preserve_exact_collisions_remaps_and_invalid_minima() {
        let mut a = LocalEventIds::new(0, 2);
        let mut b = LocalEventIds::new(3, 2);
        let ax = a.intern(b"shared-long-event");
        let bx = b.intern(b"shared-long-event");
        let ad = a.intern("Ω-dead".as_bytes());
        let bd = b.intern("Ω-dead".as_bytes());
        let distinct = b.intern(b"Shared-long-event");
        let mut event_remaps = EventIdRemaps::merge(vec![a.names, vec![], vec![], b.names], 2);
        event_remaps.force_fallback_hashes(mix64(u64::MAX));
        let people = vec![
            vec![named_person(0)],
            vec![],
            vec![],
            vec![named_person(0), named_person(1)],
        ];
        let make = |id, person, ingest, _hash: u64| Raw {
            id,
            person,
            ingest,
            ts: ingest,
            revenue: 1,
            category: 0,
        };
        let chunks = vec![
            vec![
                make(ax, LOCAL_PERSON, 50, 42),
                make(ad, LOCAL_PERSON, 40, 42),
                make(u64::MAX, u64::MAX, 20, 0),
            ],
            vec![],
            vec![make(distinct, LOCAL_PERSON | (1 << 2) | 3, 30, 42)],
            vec![
                make(bx, LOCAL_PERSON | 3, 10, 42),
                make(bd, INVALID_PERSON, 1, 42),
            ],
            vec![make(
                inline_person::encode(b"finger").unwrap(),
                u64::MAX,
                25,
                0x1_0000_002a,
            )],
        ];
        let mut expected = vec![
            (named_person(0), 10),
            (named_person(1), 30),
            (u64::MAX, 20),
            (u64::MAX, 25),
        ];
        expected.sort_unstable();
        for linear in [false, true] {
            let mut output = empty_parts(3);
            assert!(if linear {
                dedup_chunks_slots::<true>(&chunks, &event_remaps, &people, 2, &mut output)
            } else {
                dedup_chunks_slots::<false>(&chunks, &event_remaps, &people, 2, &mut output)
            });
            let mut actual = Vec::new();
            for (partition, events) in output.iter().enumerate() {
                for event in events {
                    assert_eq!(partition, person_partition(event.person, 3));
                    actual.push((event.person, event.ts));
                }
            }
            actual.sort_unstable();
            assert_eq!(actual, expected);
        }
    }
    #[test]
    fn uneven_chunks_match_independent_minimum_selection() {
        let mut chunks = vec![Vec::new(); 8];
        let mut earliest = std::collections::BTreeMap::new();
        for i in 0..20000usize {
            let id = (i * 193) % 3000;
            let ingest = ((i * 881) % 21001) as i64;
            let row = Raw {
                id: inline_person::encode(format!("id-{id}").as_bytes()).unwrap(),
                person: if i % 19 == 0 {
                    INVALID_PERSON
                } else {
                    u64::MAX
                },
                ingest,
                ts: i as i64,
                revenue: i as u32,
                category: 0,
            };
            let slot = earliest.entry(id).or_insert(row);
            if ingest < slot.ingest {
                *slot = row;
            }
            chunks[if i % 3 == 0 { 0 } else { (i * 11) % 7 + 1 }].push(row);
        }
        let mut expected: Vec<_> = earliest
            .values()
            .filter(|r| r.person != INVALID_PERSON)
            .map(|r| (r.ts, r.revenue))
            .collect();
        expected.sort_unstable();
        for linear in [false, true] {
            let mut output = vec![Vec::new()];
            assert!(if linear {
                dedup_chunks_slots::<true>(&chunks, &EventIdRemaps::default(), &[], 0, &mut output)
            } else {
                dedup_chunks_slots::<false>(&chunks, &EventIdRemaps::default(), &[], 0, &mut output)
            });
            let mut actual: Vec<_> = output[0].iter().map(|e| (e.ts, e.revenue)).collect();
            actual.sort_unstable();
            assert_eq!(actual, expected);
        }
        for chunks in [vec![], vec![vec![]], vec![vec![]; 8]] {
            let mut output = vec![Vec::new()];
            assert!(dedup_chunks_slots::<true>(
                &chunks,
                &EventIdRemaps::default(),
                &[],
                0,
                &mut output
            ));
            assert!(dedup_chunks_slots::<false>(
                &chunks,
                &EventIdRemaps::default(),
                &[],
                0,
                &mut output
            ));
            assert!(output[0].is_empty());
        }
    }
}

#[cfg(test)]
mod identity_entry_tests {
    use super::*;
    #[test]
    fn identity_entries_reuse_both_domains_and_preserve_raw_minima() {
        let long = "long-person-Ω".as_bytes();
        let other = b"another-long-person".as_slice();
        let maximum = [127u8; 9];
        let links: [(&[u8], &[u8]); 10] = [
            (b"z", long),
            (long, b"a"),
            (b"z", b"z"),
            (b"a", b"\0"),
            (long, b"z"),
            (b"\0", long),
            (other, &maximum),
            (other, other),
            (&maximum, other),
            (&maximum, &maximum),
        ];
        let mut bytes = b"user_id_a,user_id_b\n".to_vec();
        for (left, right) in links {
            bytes.extend_from_slice(left);
            bytes.push(b',');
            bytes.extend_from_slice(right);
            bytes.push(b'\n');
        }
        let mut map = memmap2::MmapMut::map_anon(bytes.len()).unwrap();
        map.copy_from_slice(&bytes);
        let files = [InputFile::Mapped(map.make_read_only().unwrap())];
        let identity = identities(&files);
        assert_eq!(identity.names.len(), 6);
        assert_eq!(identity.inline_ids.len(), 4);
        assert_eq!(identity.ids.len(), 2);
        let minimum = inline_person::encode(b"\0").unwrap();
        for name in [b"z".as_slice(), b"a", b"\0", long] {
            assert_eq!(identity.lookup(name), Some(minimum));
        }
        let named = identity.lookup(other).unwrap();
        assert_eq!(named & PERSON_TAG, NAMED_PERSON);
        assert_eq!(identity.names[(named & PERSON_PAYLOAD) as usize], other);
        assert_eq!(identity.lookup(&maximum), Some(named));
        assert_eq!(
            identity.lookup(b"unlinked"),
            inline_person::encode(b"unlinked")
        );
        assert_eq!(identity.lookup(b"unlinked-long-name"), None);
    }
}

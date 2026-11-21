//! Exact native CSV, session, attribution, and Parquet engine.
mod parquet_output;
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

#[cfg(feature = "fast-csv")]
mod fast_csv;

const DAY: i64 = 86_400_000;
const WEEK: i64 = 7 * DAY;
const GAP: i64 = 1_800_000;
const CHUNK: usize = 4 * 1024 * 1024;
type Map<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

// Hashes only choose buckets and accelerate ordering. Every equality decision
// also compares the original bytes, including collisions in these hashes.
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

#[cfg(feature = "fast-hash")]
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

#[cfg(feature = "fast-csv")]
#[inline]
fn rows<const N: usize>(bytes: &[u8]) -> fast_csv::Rows<'_, N> {
    fast_csv::Rows::new(bytes)
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
    ids: Map<&'a [u8], usize>,
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
    let mut ids = Map::with_capacity_and_hasher(reserve, Default::default());
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
                if let Some(&i) = ids.get(name) {
                    return i;
                }
                let i = names.len();
                ids.insert(name, i);
                names.push(name);
                parents.push(i);
                sizes.push(1usize);
                minima.push(i);
                i
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
    for i in ids.values_mut() {
        *i = minima[root(&mut parents, *i)];
    }
    Identity { ids, names }
}
impl<'a> Identity<'a> {
    #[inline]
    fn person(&self, id: &'a [u8]) -> &'a [u8] {
        self.ids.get(id).map_or(id, |&i| self.names[i])
    }
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
// tombstones (empty person) until the earliest ingest is selected globally.
struct Raw<'a> {
    hash: u64,
    id: &'a [u8],
    person: &'a [u8],
    ingest: i64,
    ts: i64,
    revenue: u32,
    category: u32,
}
#[derive(Clone, Copy)]
struct Event<'a> {
    hash: u64,
    person: &'a [u8],
    ts: i64,
    revenue: u32,
    category: u32,
}
#[derive(Clone, Copy)]
struct Click<'a> {
    hash: u64,
    person: &'a [u8],
    id: &'a [u8],
    ts: i64,
    campaign: u32,
}
fn empty_parts<T>(count: usize) -> Vec<Vec<T>> {
    (0..count).map(|_| Vec::new()).collect()
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
) -> Vec<Vec<Vec<Raw<'a>>>> {
    let next = AtomicUsize::new(0);
    let workers = thread::scope(|s| {
        let handles: Vec<_> = (0..ncpu)
            .map(|_| {
                s.spawn(|| {
                    let mut result = empty_parts(partitions);
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(bytes) = tasks.get(i) else {
                            break;
                        };
                        for f in rows::<7>(bytes) {
                            if f[0].is_empty() {
                                continue;
                            }
                            let Some(ingest) = timestamp(f[3]) else {
                                continue;
                            };
                            let hash = hash_bytes(f[0]);
                            let (person, ts, revenue, category) =
                                validate_fields(f, ingest, identity, catalog, start, end)
                                    .unwrap_or((&[], 0, 0, 0));
                            result[(hash as usize) % partitions].push(Raw {
                                hash,
                                id: f[0],
                                person,
                                ingest,
                                ts,
                                revenue,
                                category,
                            });
                        }
                    }
                    result
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    transpose(workers, partitions)
}
#[inline]
fn raw_id(raw: Raw<'_>) -> &[u8] {
    raw.id
}
#[inline]
fn accepted(raw: Raw<'_>) -> Option<Event<'_>> {
    if raw.person.is_empty() {
        return None;
    }
    Some(Event {
        hash: hash_bytes(raw.person),
        person: raw.person,
        ts: raw.ts,
        revenue: raw.revenue,
        category: raw.category,
    })
}
fn validate_fields<'a>(
    f: [&'a [u8]; 7],
    ingest: i64,
    identity: &Identity<'a>,
    catalog: &Catalog<'a>,
    start: i64,
    end: i64,
) -> Option<(&'a [u8], i64, u32, u32)> {
    if f[1].is_empty() || f[5].is_empty() {
        return None;
    }
    let ts = timestamp(f[2])?;
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
    Some((identity.person(f[1]), ts, revenue, category))
}
fn dedup_events<'a>(
    inputs: Vec<Vec<Vec<Raw<'a>>>>,
    ncpu: usize,
    partitions: usize,
) -> Vec<Vec<Vec<Event<'a>>>> {
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
                        rows.sort_unstable_by_key(|r| r.hash);
                        let mut pos = 0;
                        while pos < rows.len() {
                            let mut last = pos + 1;
                            while last < rows.len() && rows[last].hash == rows[pos].hash {
                                last += 1;
                            }
                            if last == pos + 1 {
                                if let Some(e) = accepted(rows[pos]) {
                                    output[(e.hash as usize) % partitions].push(e);
                                }
                            } else {
                                // Usually one event and a few duplicates. Preserve separate
                                // IDs even if their full 64-bit hashes collide.
                                let mut kept: Vec<Raw<'a>> = Vec::with_capacity(1);
                                for &r in &rows[pos..last] {
                                    let id = raw_id(r);
                                    if let Some(previous) =
                                        kept.iter_mut().find(|p| raw_id(**p) == id)
                                    {
                                        if r.ingest < previous.ingest {
                                            *previous = r;
                                        }
                                    } else {
                                        kept.push(r);
                                    }
                                }
                                for r in kept {
                                    if let Some(e) = accepted(r) {
                                        output[(e.hash as usize) % partitions].push(e);
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
                    let mut output = empty_parts(partitions);
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
                            let Some(ts) = timestamp(time) else {
                                continue;
                            };
                            if ts < start - WEEK || ts >= end {
                                continue;
                            }
                            let person = identity.person(user);
                            let hash = hash_bytes(person);
                            output[(hash as usize) % partitions].push(Click {
                                hash,
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
fn sort_events(events: &mut [Event<'_>]) {
    // Most equal hashes identify one person. Avoid comparing that same name
    // at every comparison-sort step; verify the bytes once per record instead.
    events.sort_unstable_by(|a, b| a.hash.cmp(&b.hash).then(a.ts.cmp(&b.ts)));
    let mut first = 0;
    while first < events.len() {
        let mut end = first + 1;
        while end < events.len() && events[end].hash == events[first].hash {
            end += 1;
        }
        let person = events[first].person;
        if events[first + 1..end]
            .iter()
            .any(|event| event.person != person)
        {
            // A hash collision is never treated as an identity match. Restore
            // exact person/time order for the entire colliding hash bucket.
            events[first..end]
                .sort_unstable_by(|a, b| a.person.cmp(b.person).then(a.ts.cmp(&b.ts)));
        }
        first = end;
    }
}
fn sort_clicks(clicks: &mut [Click<'_>]) {
    clicks.sort_unstable_by(|a, b| {
        a.hash
            .cmp(&b.hash)
            .then(a.ts.cmp(&b.ts))
            .then_with(|| b.id.cmp(a.id))
    });
    let mut first = 0;
    while first < clicks.len() {
        let mut end = first + 1;
        while end < clicks.len() && clicks[end].hash == clicks[first].hash {
            end += 1;
        }
        let person = clicks[first].person;
        if clicks[first + 1..end]
            .iter()
            .any(|click| click.person != person)
        {
            clicks[first..end].sort_unstable_by(|a, b| {
                a.person
                    .cmp(b.person)
                    .then(a.ts.cmp(&b.ts))
                    .then_with(|| b.id.cmp(a.id))
            });
        }
        first = end;
    }
}
fn process_partition(
    mut events: Vec<Event<'_>>,
    mut clicks: Vec<Click<'_>>,
    catalog: &Catalog<'_>,
    out: PathBuf,
    day_min: i32,
    day_count: usize,
) -> io::Result<(CampaignMap, u64)> {
    sort_events(&mut events);
    sort_clicks(&mut clicks);
    // All copies have identical values. Sorting puts them together after their
    // unquoted values and canonical people have been compared exactly.
    clicks
        .dedup_by(|a, b| a.hash == b.hash && a.person == b.person && a.ts == b.ts && a.id == b.id);
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
        let hash = events[ev].hash;
        let mut ev_end = ev + 1;
        while ev_end < events.len()
            && events[ev_end].hash == hash
            && events[ev_end].person == person
        {
            ev_end += 1;
        }
        while ck < clicks.len() && (clicks[ck].hash, clicks[ck].person) < (hash, person) {
            ck += 1;
        }
        let ck_start = ck;
        while ck < clicks.len() && clicks[ck].hash == hash && clicks[ck].person == person {
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
                flush_user(&mut writer, person, &agg, &category_sums, catalog)?;
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
        flush_user(&mut writer, person, &agg, &category_sums, catalog)?;
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
    let identity = identities(&link_data);
    let catalog = catalog(&product_data, &campaign_data);
    eprintln!(
        "identities: {} IDs, {:.3}s",
        identity.names.len(),
        began.elapsed().as_secs_f64()
    );
    let event_data = load_files(&csv_paths(&data.join("events"))?, ncpu)?;
    // Tiny fixtures need one part, not dozens of empty files. Full datasets
    // retain four independent partitions per CPU for balanced work queues.
    let event_bytes: usize = event_data.iter().map(|f| f.len()).sum();
    let partitions = (event_bytes / CHUNK).clamp(1, ncpu * 4);
    let event_tasks = chunks(&event_data);
    let raw = parse_events(
        &event_tasks,
        &identity,
        &catalog,
        start,
        end,
        ncpu,
        partitions,
    );
    let raw_count: usize = raw.iter().flat_map(|v| v.iter()).map(Vec::len).sum();
    eprintln!(
        "parsed events: {raw_count}, {:.3}s",
        began.elapsed().as_secs_f64()
    );
    let events = dedup_events(raw, ncpu, partitions);
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
    fn event_dedup_keeps_exact_ids_and_invalid_earliest_under_forced_hash_collision() {
        let later = Raw {
            hash: 42,
            id: b"x",
            person: b"u",
            ingest: 200,
            ts: 10,
            revenue: 1,
            category: 0,
        };
        let earliest_invalid = Raw {
            ingest: 100,
            person: b"",
            ..later
        };
        let other = Raw {
            hash: 42,
            id: b"y",
            person: b"v",
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
            vec![vec![vec![later, other_later, earliest_invalid, other]]],
            1,
            1,
        );
        let kept: Vec<_> = output.into_iter().flatten().flatten().collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].person, b"v");
        assert_eq!(kept[0].revenue, 7);
    }
    fn event(hash: u64, person: &'static [u8], ts: i64) -> Event<'static> {
        Event {
            hash,
            person,
            ts,
            revenue: 0,
            category: 0,
        }
    }
    fn click(hash: u64, person: &'static [u8], ts: i64, id: &'static [u8]) -> Click<'static> {
        Click {
            hash,
            person,
            id,
            ts,
            campaign: 0,
        }
    }
    #[test]
    fn event_sort_resolves_forced_person_hash_collisions() {
        let mut events = vec![
            event(7, b"b", 2),
            event(7, b"a", 4),
            event(1, b"z", 9),
            event(7, b"b", 1),
            event(7, b"A", 3),
            event(7, b"a", 0),
        ];
        sort_events(&mut events);
        let actual: Vec<_> = events.iter().map(|e| (e.hash, e.person, e.ts)).collect();
        assert_eq!(
            actual,
            vec![
                (1, &b"z"[..], 9),
                (7, &b"A"[..], 3),
                (7, &b"a"[..], 0),
                (7, &b"a"[..], 4),
                (7, &b"b"[..], 1),
                (7, &b"b"[..], 2)
            ]
        );
    }
    #[test]
    fn click_sort_resolves_forced_person_hash_collisions_and_ties() {
        let mut clicks = vec![
            click(7, b"b", 100, b"a"),
            click(7, b"a", 100, b"a"),
            click(7, b"b", 99, b"z"),
            click(7, b"a", 100, b"z"),
            click(1, b"z", 9, b"x"),
            click(7, b"b", 100, b"z"),
        ];
        sort_clicks(&mut clicks);
        let actual: Vec<_> = clicks
            .iter()
            .map(|c| (c.hash, c.person, c.ts, c.id))
            .collect();
        assert_eq!(
            actual,
            vec![
                (1, &b"z"[..], 9, &b"x"[..]),
                (7, &b"a"[..], 100, &b"z"[..]),
                (7, &b"a"[..], 100, &b"a"[..]),
                (7, &b"b"[..], 99, &b"z"[..]),
                (7, &b"b"[..], 100, &b"z"[..]),
                (7, &b"b"[..], 100, &b"a"[..])
            ]
        );
        for person in [&b"a"[..], &b"b"[..]] {
            let credit = clicks
                .iter()
                .filter(|c| c.person == person && c.ts <= 100)
                .last()
                .unwrap();
            assert_eq!(credit.id, b"a");
        }
    }
    #[test]
    fn click_sort_without_collision_credits_smallest_bytewise_id() {
        let mut clicks = vec![
            click(5, b"u", 100, b"z"),
            click(5, b"u", 101, b"after"),
            click(5, b"u", 99, b"earlier"),
            click(5, b"u", 100, b"\0"),
            click(5, b"u", 100, b"A"),
        ];
        sort_clicks(&mut clicks);
        let eligible: Vec<_> = clicks.iter().take_while(|c| c.ts <= 100).collect();
        assert_eq!(eligible.last().unwrap().id, b"\0");
        assert_eq!(eligible.last().unwrap().ts, 100);
        assert_eq!(clicks.last().unwrap().id, b"after");
    }
    #[test]
    fn hash_sort_handles_empty_and_single_record_inputs() {
        sort_events(&mut []);
        sort_clicks(&mut []);
        let mut events = [event(0, b"u", 3)];
        let mut clicks = [click(0, b"u", 3, b"c")];
        sort_events(&mut events);
        sort_clicks(&mut clicks);
        assert_eq!(events[0].ts, 3);
        assert_eq!(clicks[0].id, b"c");
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
        assert_eq!(digits("٢٥٠".as_bytes()), None);
    }
}

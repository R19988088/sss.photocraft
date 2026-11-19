//! A minimal, read-only ZIP reader: enough for Procreate brushsets, Krita resource bundles,
//! SAI2 brush packs and Corel Painter libraries (stored and deflate entries, no encryption).

use std::io::Read;

const EOCD: u32 = 0x0605_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;
/// Largest entry this reader inflates.
const MAX_ENTRY: u64 = 512 << 20;

/// One central-directory record.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    local_offset: u64,
}

/// A ZIP file parsed into its entry table, holding the bytes it reads from.
pub struct Archive<'a> {
    bytes: &'a [u8],
    pub entries: Vec<Entry>,
}

/// Is this a ZIP archive?
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06")
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn central_directory_offset(bytes: &[u8]) -> Option<(usize, usize)> {
    let tail = bytes.len().saturating_sub(65_557);
    let end = bytes.len().checked_sub(22)?;
    for i in (tail..=end).rev() {
        if u32_at(bytes, i) != Some(EOCD) {
            continue;
        }
        let count = u16_at(bytes, i + 10)? as usize;
        let offset = u32_at(bytes, i + 16)? as usize;
        return Some((count, offset));
    }
    None
}

/// Parse the archive's entry table.
pub fn open(bytes: &[u8]) -> Result<Archive<'_>, String> {
    let (count, offset) = central_directory_offset(bytes).ok_or("the ZIP end-of-directory record is missing")?;
    let mut entries = Vec::with_capacity(count.min(10_000));
    let mut p = offset;
    for _ in 0..count {
        if u32_at(bytes, p) != Some(CENTRAL) {
            return Err("the ZIP central directory is damaged".into());
        }
        let method = u16_at(bytes, p + 10).unwrap_or(0);
        let compressed_size = u64::from(u32_at(bytes, p + 20).unwrap_or(0));
        let uncompressed_size = u64::from(u32_at(bytes, p + 24).unwrap_or(0));
        let name_len = u16_at(bytes, p + 28).unwrap_or(0) as usize;
        let extra_len = u16_at(bytes, p + 30).unwrap_or(0) as usize;
        let comment_len = u16_at(bytes, p + 32).unwrap_or(0) as usize;
        let local_offset = u64::from(u32_at(bytes, p + 42).unwrap_or(0));
        let name = bytes.get(p + 46..p + 46 + name_len).ok_or("the ZIP entry name is damaged")?;
        let name = String::from_utf8_lossy(name).replace('\\', "/");
        entries.push(Entry { name, method, compressed_size, uncompressed_size, local_offset });
        p = p.checked_add(46 + name_len + extra_len + comment_len).ok_or("the ZIP central directory overflows")?;
    }
    Ok(Archive { bytes, entries })
}

impl Archive<'_> {
    /// Entry names, in archive order.
    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// The first entry whose name ends with `suffix` (case-insensitive).
    pub fn find_suffix(&self, suffix: &str) -> Option<&Entry> {
        let suffix = suffix.to_ascii_lowercase();
        self.entries.iter().find(|e| e.name.to_ascii_lowercase().ends_with(&suffix))
    }

    /// The entry with exactly this name.
    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Inflate (or copy) one entry.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>, String> {
        if entry.uncompressed_size > MAX_ENTRY || entry.compressed_size > MAX_ENTRY {
            return Err(format!("{} is too large for a brush resource", entry.name));
        }
        let local = entry.local_offset as usize;
        if u32_at(self.bytes, local) != Some(LOCAL) {
            return Err(format!("{}: the ZIP local header is damaged", entry.name));
        }
        let name_len = u16_at(self.bytes, local + 26).ok_or("the ZIP local header is truncated")? as usize;
        let extra_len = u16_at(self.bytes, local + 28).ok_or("the ZIP local header is truncated")? as usize;
        let start = local + 30 + name_len + extra_len;
        let raw = self.bytes.get(start..start + entry.compressed_size as usize).ok_or_else(|| format!("{}: the ZIP data is truncated", entry.name))?;
        match entry.method {
            0 => Ok(raw.to_vec()),
            8 => {
                let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
                flate2::read::DeflateDecoder::new(raw)
                    .take(MAX_ENTRY)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("{}: {e}", entry.name))?;
                Ok(out)
            }
            m => Err(format!("{}: compression method {m} is not supported", entry.name)),
        }
    }

    /// Read an entry by exact name.
    pub fn read_name(&self, name: &str) -> Result<Vec<u8>, String> {
        let entry = self.find(name).ok_or_else(|| format!("{name} is missing from the archive"))?;
        self.read(entry)
    }

    /// Read the first entry whose name ends with `suffix`.
    pub fn read_suffix(&self, suffix: &str) -> Result<Vec<u8>, String> {
        let entry = self.find_suffix(suffix).ok_or_else(|| format!("{suffix} is missing from the archive"))?;
        self.read(entry)
    }
}

/// Entry names of a ZIP, for format sniffing.
pub fn names(bytes: &[u8]) -> Vec<String> {
    open(bytes).map(|a| a.entries.into_iter().map(|e| e.name).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(v: &mut Vec<u8>, n: u16) {
        v.extend_from_slice(&n.to_le_bytes());
    }
    fn put32(v: &mut Vec<u8>, n: u32) {
        v.extend_from_slice(&n.to_le_bytes());
    }

    fn stored(name: &str, data: &[u8]) -> Vec<u8> {
        let crc = crc32fast::hash(data);
        let mut out = Vec::new();
        put32(&mut out, LOCAL);
        put16(&mut out, 10);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put32(&mut out, crc);
        put32(&mut out, data.len() as u32);
        put32(&mut out, data.len() as u32);
        put16(&mut out, name.len() as u16);
        put16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        out
    }

    fn central(name: &str, data: &[u8], offset: u32) -> Vec<u8> {
        let mut out = Vec::new();
        put32(&mut out, CENTRAL);
        put16(&mut out, 20);
        put16(&mut out, 10);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put32(&mut out, crc32fast::hash(data));
        put32(&mut out, data.len() as u32);
        put32(&mut out, data.len() as u32);
        put16(&mut out, name.len() as u16);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put32(&mut out, 0);
        put32(&mut out, offset);
        out.extend_from_slice(name.as_bytes());
        out
    }

    #[test]
    fn reads_stored_entries() {
        let a = stored("Brush.archive", b"hello");
        let b = stored("Shape.png", b"png!");
        let mut file = [a, b].concat();
        let cd_offset = file.len() as u32;
        let cd = [central("Brush.archive", b"hello", 0), central("Shape.png", b"png!", 30 + 13 + 5)].concat();
        let cd_size = cd.len() as u32;
        file.extend_from_slice(&cd);
        put32(&mut file, EOCD);
        put16(&mut file, 0);
        put16(&mut file, 0);
        put16(&mut file, 2);
        put16(&mut file, 2);
        put32(&mut file, cd_size);
        put32(&mut file, cd_offset);
        put16(&mut file, 0);
        let archive = open(&file).unwrap();
        assert_eq!(archive.names(), ["Brush.archive", "Shape.png"]);
        assert_eq!(archive.read_name("Brush.archive").unwrap(), b"hello");
        assert_eq!(archive.read_suffix(".png").unwrap(), b"png!");
    }
}

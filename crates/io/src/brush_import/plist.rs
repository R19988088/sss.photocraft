//! Binary property lists (`bplist00`), including the `NSKeyedArchiver` shape Procreate writes
//! (`$archiver`/`$objects`/`$top`), decoded into plain JSON values.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

const MAX_DEPTH: usize = 64;
/// Most nodes one archive may expand into; wider sharing means it is a graph, not a value tree.
const MAX_NODES: usize = 200_000;
const MAX_OBJECTS: usize = 2_000_000;

/// Is this a binary plist?
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.len() >= 40 && bytes.starts_with(b"bplist")
}

fn uint(b: &[u8], o: usize, n: usize) -> Option<u64> {
    if n == 0 || n > 8 {
        return None;
    }
    let s = b.get(o..o + n)?;
    let mut v = 0u64;
    for x in s {
        v = v.checked_mul(256)?.checked_add(u64::from(*x))?;
    }
    Some(v)
}

fn sint(b: &[u8], o: usize, n: usize) -> Option<i64> {
    let s = b.get(o..o + n)?;
    let mut v = 0i64;
    for x in s {
        v = v.wrapping_mul(256).wrapping_add(i64::from(*x));
    }
    Some(v)
}

#[derive(Clone, Debug)]
enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Str(String),
    Data(Vec<u8>),
    Uid(u64),
    Array(Vec<u64>),
    Dict(Vec<(u64, u64)>),
}

struct Reader<'a> {
    b: &'a [u8],
    offsets: Vec<usize>,
    refs: usize,
    top: usize,
    /// `$objects` of a keyed archive: UID *n* means the object at `objects[n]` (an index into the
    /// offset table, not the object itself — the two orders differ).
    objects: Vec<u64>,
}

impl<'a> Reader<'a> {
    fn open(b: &'a [u8]) -> Result<Self, String> {
        if !is_binary(b) {
            return Err("not a binary plist".into());
        }
        let trailer = b.len() - 32;
        let offset_size = b[trailer + 6] as usize;
        let ref_size = b[trailer + 7] as usize;
        let count = uint(b, trailer + 8, 8).ok_or("the plist object count is damaged")? as usize;
        let top = uint(b, trailer + 16, 8).ok_or("the plist top object is damaged")? as usize;
        let table = uint(b, trailer + 24, 8).ok_or("the plist offset table is damaged")? as usize;
        if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) || count > MAX_OBJECTS || top >= count {
            return Err("the plist header is invalid".into());
        }
        let end = table.checked_add(count.checked_mul(offset_size).ok_or("the plist offset table overflows")?).ok_or("the plist offset table overflows")?;
        if end > b.len() {
            return Err("the plist offset table runs past the file".into());
        }
        let mut offsets = Vec::with_capacity(count);
        for i in 0..count {
            offsets.push(uint(b, table + i * offset_size, offset_size).ok_or("the plist offset is damaged")? as usize);
        }
        Ok(Reader { b, offsets, refs: ref_size, top, objects: Vec::new() })
    }

    fn length(&self, at: &mut usize, info: u8) -> Result<usize, String> {
        if info != 0x0f {
            return Ok(info as usize);
        }
        let marker = *self.b.get(*at).ok_or("the plist length is truncated")?;
        *at += 1;
        let n = 1usize << (marker & 0x0f);
        if n > 8 {
            return Err("the plist length field is too wide".into());
        }
        let v = uint(self.b, *at, n).ok_or("the plist length is truncated")? as usize;
        *at += n;
        Ok(v)
    }

    fn object(&self, idx: usize) -> Result<Obj, String> {
        let off = *self.offsets.get(idx).ok_or("the plist object index is out of range")?;
        let marker = *self.b.get(off).ok_or("the plist object offset is out of range")?;
        let kind = marker >> 4;
        let info = marker & 0x0f;
        let mut p = off + 1;
        Ok(match kind {
            0x0 => match info {
                0x08 => Obj::Bool(false),
                0x09 => Obj::Bool(true),
                _ => Obj::Null,
            },
            0x1 => Obj::Int(sint(self.b, p, 1usize << info).ok_or("the plist integer is truncated")?),
            0x2 => {
                let n = 1usize << info;
                if n == 4 {
                    Obj::Real(f32::from_be_bytes(self.b.get(p..p + 4).ok_or("the plist float is truncated")?.try_into().map_err(|_| "the plist float is truncated")?) as f64)
                } else {
                    Obj::Real(f64::from_be_bytes(self.b.get(p..p + 8).ok_or("the plist double is truncated")?.try_into().map_err(|_| "the plist double is truncated")?))
                }
            }
            0x4 => {
                let len = self.length(&mut p, info)?;
                Obj::Data(self.b.get(p..p + len).ok_or("the plist data is truncated")?.to_vec())
            }
            0x5 => {
                let len = self.length(&mut p, info)?;
                let s = self.b.get(p..p + len).ok_or("the plist string is truncated")?;
                let cs: Vec<u8> = s.iter().copied().filter(|b| *b != 0).collect();
                Obj::Str(String::from_utf8_lossy(&cs).to_string())
            }
            0x6 => {
                let len = self.length(&mut p, info)?;
                let s = self.b.get(p..p + len * 2).ok_or("the plist UTF-16 string is truncated")?;
                let chars: Vec<u16> = s.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                Obj::Str(String::from_utf16_lossy(&chars))
            }
            0x8 => Obj::Uid(uint(self.b, p, info as usize + 1).ok_or("the plist UID is truncated")?),
            0xa | 0xb | 0xc => {
                let len = self.length(&mut p, info)?;
                let mut refs = Vec::with_capacity(len.min(100_000));
                for i in 0..len {
                    refs.push(uint(self.b, p + i * self.refs, self.refs).ok_or("the plist array is truncated")?);
                }
                Obj::Array(refs)
            }
            0xd => {
                let len = self.length(&mut p, info)?;
                let mut pairs = Vec::with_capacity(len.min(100_000));
                for i in 0..len {
                    let k = uint(self.b, p + i * self.refs, self.refs).ok_or("the plist dictionary is truncated")?;
                    let v = uint(self.b, p + (len + i) * self.refs, self.refs).ok_or("the plist dictionary is truncated")?;
                    pairs.push((k, v));
                }
                Obj::Dict(pairs)
            }
            _ => return Err(format!("plist object kind {kind:#x} is not supported")),
        })
    }

    fn key(&self, idx: u64) -> Option<String> {
        match self.object(idx as usize).ok()? {
            Obj::Str(s) => Some(s),
            _ => None,
        }
    }

    fn entry(&self, idx: u64) -> Obj {
        self.object(idx as usize).unwrap_or(Obj::Null)
    }

    fn resolve(&self, v: &Obj) -> Value {
        // A keyed archive may reference the same object from several places, and Procreate's
        // archives contain back-references (parent pointers, NSUUID tables) that form cycles.
        // Resolving them by Uid alone expands exponentially; the ancestor set stops a cycle and
        // the node budget stops a wide sharing graph from ballooning (#brushset-hang).
        let mut path = std::collections::BTreeSet::new();
        let mut budget = MAX_NODES;
        self.resolve_node(v, 0, &mut path, &mut budget)
    }

    fn resolve_node(&self, v: &Obj, depth: usize, path: &mut std::collections::BTreeSet<u64>, budget: &mut usize) -> Value {
        if depth > MAX_DEPTH || *budget == 0 {
            return Value::Null;
        }
        *budget -= 1;
        match v {
            Obj::Null => Value::Null,
            Obj::Bool(b) => Value::Bool(*b),
            Obj::Int(i) => Value::from(*i),
            Obj::Real(r) => serde_json::Number::from_f64(*r).map_or(Value::Null, Value::Number),
            Obj::Str(s) => Value::String(s.clone()),
            Obj::Data(d) => Value::String(photocraft_paint::tile::b64_encode(d)),
            Obj::Uid(u) => {
                if !path.insert(*u) {
                    return Value::Null;
                }
                let idx = self.objects.get(*u as usize).copied().unwrap_or(*u);
                let o = self.entry(idx);
                let out = self.resolve_node(&o, depth + 1, path, budget);
                path.remove(u);
                out
            }
            Obj::Array(refs) => Value::Array(
                refs.iter()
                    .map(|r| {
                        if !path.insert(*r) {
                            return Value::Null;
                        }
                        let out = self.resolve_node(&self.entry(*r), depth + 1, path, budget);
                        path.remove(r);
                        out
                    })
                    .collect(),
            ),
            Obj::Dict(pairs) => {
                let mut map = Map::new();
                for (k, v) in pairs {
                    if let Some(key) = self.key(*k)
                        && key != "$class"
                    {
                        let value = if !path.insert(*v) {
                            Value::Null
                        } else {
                            let out = self.resolve_node(&self.entry(*v), depth + 1, path, budget);
                            path.remove(v);
                            out
                        };
                        map.insert(key, value);
                    }
                }
                Value::Object(map)
            }
        }
    }
}

/// Parse a binary plist (plain or `NSKeyedArchiver`) into JSON.
pub fn parse(bytes: &[u8]) -> Result<Value, String> {
    let mut reader = Reader::open(bytes)?;
    let top = reader.object(reader.top)?;
    if let Obj::Dict(pairs) = &top {
        let objects = pairs.iter().find(|(k, _)| reader.key(*k).as_deref() == Some("$objects")).map(|(_, v)| *v);
        let root = pairs.iter().find(|(k, _)| reader.key(*k).as_deref() == Some("$top")).map(|(_, v)| *v);
        if let Some((objects_ref, root_ref)) = objects.zip(root) {
            // `$top` maps "root" to a UID object; the UID's value is the index into `$objects`.
            let root_ref = match reader.object(root_ref as usize) {
                Ok(Obj::Dict(p)) => p.iter().find(|(k, _)| reader.key(*k).as_deref() == Some("root")).map(|(_, v)| *v),
                _ => None,
            };
            let root_index = match root_ref {
                Some(i) => match reader.object(i as usize) {
                    Ok(Obj::Uid(u)) => u,
                    _ => i,
                },
                None => 1,
            };
            let array = match reader.object(objects_ref as usize) {
                Ok(Obj::Array(a)) => a,
                _ => return Err("the keyed archive has no object table".into()),
            };
            let value = array.get(root_index as usize).map(|i| reader.entry(*i)).unwrap_or(Obj::Null);
            reader.objects = array;
            return Ok(reader.resolve(&value));
        }
    }
    Ok(reader.resolve(&top))
}

/// Parse the XML form of a plist (only the keys Procreate's `brushset.plist` uses).
pub fn parse_xml_keys(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut rest = text;
    while let Some(at) = rest.find("<key>") {
        let tail = &rest[at + 5..];
        let Some(kend) = tail.find("</key>") else { break };
        let key = tail[..kend].trim().to_string();
        let after = tail[kend + 6..].trim_start();
        if let Some(list) = after.strip_prefix("<array>") {
            let Some(aend) = list.find("</array>") else { break };
            let mut items = Vec::new();
            let mut inner = &list[..aend];
            while let Some(s) = inner.find("<string>") {
                let t = &inner[s + 8..];
                let Some(e) = t.find("</string>") else { break };
                items.push(t[..e].trim().to_string());
                inner = &t[e + 9..];
            }
            out.insert(key, items);
            rest = &list[aend..];
        } else if let Some(v) = after.strip_prefix("<string>") {
            let Some(e) = v.find("</string>") else { break };
            out.insert(key, vec![v[..e].trim().to_string()]);
            rest = &v[e..];
        } else {
            rest = after;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny three-object binary plist: {"name": "Ink"}.
    fn sample() -> Vec<u8> {
        let mut b = b"bplist00".to_vec();
        b.extend_from_slice(&[0xd1, 0x01, 0x02]);
        b.extend_from_slice(&[0x54, b'n', b'a', b'm', b'e']);
        b.extend_from_slice(&[0x53, b'I', b'n', b'k']);
        let table = b.len();
        b.extend_from_slice(&[8, 11, 16]);
        b.extend_from_slice(&[0u8; 6]);
        b.push(1);
        b.push(1);
        b.extend_from_slice(&3u64.to_be_bytes());
        b.extend_from_slice(&0u64.to_be_bytes());
        b.extend_from_slice(&(table as u64).to_be_bytes());
        b
    }

    #[test]
    fn reads_a_plain_binary_plist() {
        let v = parse(&sample()).unwrap();
        assert_eq!(v["name"], "Ink");
    }

    /// A keyed archive: `$objects` is a table of object indices, and a UID *n* means the object at
    /// `$objects[n]` — not the object-table entry *n* (the two orders differ in real files).
    fn keyed_sample() -> Vec<u8> {
        let mut body = b"bplist00".to_vec();
        let mut offsets: Vec<u16> = Vec::new();
        let mut put = |bytes: &[u8], body: &mut Vec<u8>, offsets: &mut Vec<u16>| {
            offsets.push(body.len() as u16);
            body.extend_from_slice(bytes);
        };
        let s = |text: &str| {
            let mut v = vec![0x50 | text.len() as u8];
            v.extend_from_slice(text.as_bytes());
            v
        };
        put(&[0xd3, 1, 2, 3, 4, 8, 9], &mut body, &mut offsets); // 0: the archive dict
        put(&s("$archiver"), &mut body, &mut offsets); // 1
        put(&s("$objects"), &mut body, &mut offsets); // 2
        put(&s("$top"), &mut body, &mut offsets); // 3
        put(&s("NSKeyedArchiver"), &mut body, &mut offsets); // 4
        put(&s("$null"), &mut body, &mut offsets); // 5
        put(&s("PW-ink"), &mut body, &mut offsets); // 6
        put(&[0xd1, 10, 11], &mut body, &mut offsets); // 7: {name: uid 1}
        put(&[0xa3, 5, 6, 7], &mut body, &mut offsets); // 8: $objects
        put(&[0xd1, 12, 13], &mut body, &mut offsets); // 9: $top
        put(&s("name"), &mut body, &mut offsets); // 10
        put(&[0x80, 1], &mut body, &mut offsets); // 11: uid 1
        put(&s("root"), &mut body, &mut offsets); // 12
        put(&[0x80, 2], &mut body, &mut offsets); // 13: uid 2
        let table = body.len() as u64;
        for off in &offsets {
            body.extend_from_slice(&off.to_be_bytes());
        }
        body.extend_from_slice(&[0u8; 6]);
        body.push(2); // offset size
        body.push(1); // reference size
        body.extend_from_slice(&(offsets.len() as u64).to_be_bytes());
        body.extend_from_slice(&0u64.to_be_bytes());
        body.extend_from_slice(&table.to_be_bytes());
        body
    }

    #[test]
    fn a_keyed_archive_resolves_uids_through_the_object_table() {
        let v = parse(&keyed_sample()).unwrap();
        assert_eq!(v["name"], "PW-ink");
    }

    #[test]
    fn a_reference_cycle_terminates_instead_of_exploding() {
        // {"a": [{"a": [...]}]}: object 0 is the dict, 1 the key "a", 2 an array holding uid 0.
        let mut b = b"bplist00".to_vec();
        b.extend_from_slice(&[0xd1, 0x01, 0x02]);
        b.extend_from_slice(&[0x51, b'a']);
        b.extend_from_slice(&[0xa1, 0x00]);
        let table = b.len();
        b.extend_from_slice(&[8, 11, 13]);
        b.extend_from_slice(&[0u8; 6]);
        b.push(1);
        b.push(1);
        b.extend_from_slice(&3u64.to_be_bytes());
        b.extend_from_slice(&0u64.to_be_bytes());
        b.extend_from_slice(&(table as u64).to_be_bytes());
        let v = parse(&b).unwrap();
        assert_eq!(v["a"][0]["a"], Value::Null, "the cycle stops at its ancestor");
    }

    #[test]
    fn reads_xml_keys() {
        let xml = "<plist><dict><key>name</key><string>Set</string><key>brushes</key><array><string>A</string><string>B</string></array></dict></plist>";
        let keys = parse_xml_keys(xml);
        assert_eq!(keys["name"], ["Set"]);
        assert_eq!(keys["brushes"], ["A", "B"]);
    }
}

//! GIMP brushes: `.gbr` (one tip) and `.gih` (a "brush pipe" of cells).
//!
//! Header (gimpbrush-header.h), big-endian: header size, version, width, height, bytes per
//! pixel, then (version ≥ 2) the `GIMP` magic, spacing, and a NUL-terminated name; the pixels
//! follow. Version 1 headers have no magic/spacing. A `.gih` starts with two text lines — the
//! pipe's name and `<count> <cells> <cols> <rows> …` — then that many `.gbr`-style cells.

use super::{ImportedBrush, ParsedFile};

const MAGIC: u32 = 0x4749_4d50;
const MAX_DIM: u32 = 10_000;

/// A decoded header at `pos`.
struct Header {
    width: u32,
    height: u32,
    bpp: u32,
    spacing: u32,
    name: String,
    data_start: usize,
}

fn u32_be(b: &[u8], o: usize) -> Option<u32> {
    let s = b.get(o..o + 4)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn header(bytes: &[u8], pos: usize) -> Option<Header> {
    let header_size = u32_be(bytes, pos)?;
    let version = u32_be(bytes, pos + 4)?;
    if !(1..=3).contains(&version) {
        return None;
    }
    let minimum = if version == 1 { 20 } else { 28 };
    if header_size < minimum || header_size as usize > bytes.len() - pos {
        return None;
    }
    let width = u32_be(bytes, pos + 8)?;
    let height = u32_be(bytes, pos + 12)?;
    let bpp = u32_be(bytes, pos + 16)?;
    if width == 0 || height == 0 || width > MAX_DIM || height > MAX_DIM {
        return None;
    }
    if version >= 2 && u32_be(bytes, pos + 20)? != MAGIC {
        return None;
    }
    if !matches!(bpp, 1 | 2 | 4) {
        return None;
    }
    let (spacing, name_start) = if version >= 2 { (u32_be(bytes, pos + 24)?, pos + 28) } else { (10, pos + 20) };
    let name_len = header_size as usize - (name_start - pos);
    let raw = bytes.get(name_start..name_start.saturating_add(name_len)).unwrap_or_default();
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    Some(Header { width, height, bpp, spacing, name: String::from_utf8_lossy(&raw[..end]).to_string(), data_start: pos + header_size as usize })
}

/// Is this a `.gbr`?
pub fn is_gbr(bytes: &[u8]) -> bool {
    header(bytes, 0).is_some()
}

fn first_line(bytes: &[u8], from: usize) -> Option<(String, usize)> {
    let end = bytes.get(from..)?;
    let end = end.iter().take(1024).position(|b| *b == b'\n')? + from;
    Some((String::from_utf8_lossy(&bytes[from..end]).trim().to_string(), end + 1))
}

/// Is this a `.gih`? Its first text line is a name, the second a count, then a valid cell.
pub fn is_gih(bytes: &[u8]) -> bool {
    let Some((_, after)) = first_line(bytes, 0) else { return false };
    let Some((count_line, after2)) = first_line(bytes, after) else { return false };
    count_line.split_whitespace().next().and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| n > 0 && n <= 1000)
        && header(bytes, after2).is_some()
}

fn tip_of(bytes: &[u8], h: &Header) -> Option<(Vec<u8>, u32, u32)> {
    let count = (h.width as usize).checked_mul(h.height as usize)?;
    let need = count.checked_mul(h.bpp as usize)?;
    let src = bytes.get(h.data_start..h.data_start.checked_add(need)?)?;
    let mut luma = vec![0u8; count];
    match h.bpp {
        1 => luma.copy_from_slice(src),
        2 => {
            for (i, out) in luma.iter_mut().enumerate() {
                *out = src[i * 2];
            }
        }
        _ => {
            for (i, out) in luma.iter_mut().enumerate() {
                *out = src[i * 4 + 3];
            }
        }
    }
    Some((luma, h.width, h.height))
}

fn brush(h: &Header, tip: Option<(Vec<u8>, u32, u32)>, name: &str) -> ImportedBrush {
    let tip = tip.map(|(luma, w, hh)| super::cropped_tile(w, hh, &luma));
    ImportedBrush {
        name: name.to_string(),
        size: tip.as_ref().map_or(40.0, |t| t.width.max(t.height) as f32),
        spacing: if h.spacing == 0 { 0.10 } else { (h.spacing as f32 / 100.0).clamp(0.01, 10.0) },
        tip,
        hardness: 1.0,
        roundness: 1.0,
        ..Default::default()
    }
}

/// Parse `.gbr` or `.gih`.
pub fn parse(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    if is_gbr(bytes) && !is_gih(bytes) {
        return parse_gbr(bytes, file_name);
    }
    if is_gih(bytes) {
        return parse_gih(bytes, file_name);
    }
    Err("not a readable GIMP brush (.gbr/.gih)".into())
}

fn stem(name: &str) -> String {
    std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().trim().to_string()).unwrap_or_default()
}

fn parse_gbr(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    let h = header(bytes, 0).ok_or("the .gbr header is incomplete")?;
    let mut warnings = Vec::new();
    let tip = tip_of(bytes, &h);
    if tip.is_none() {
        warnings.push("the brush pixel data is incomplete".to_string());
    }
    let name = if h.name.trim().is_empty() { stem(file_name) } else { h.name.clone() };
    Ok(ParsedFile { brushes: vec![brush(&h, tip, &name)], warnings })
}

fn parse_gih(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    let (pipe_name, mut pos) = first_line(bytes, 0).ok_or("the .gih text header is missing")?;
    let (line2, next) = first_line(bytes, pos).ok_or("the .gih brush count line is missing")?;
    pos = next;
    let count: usize = line2.split_whitespace().next().and_then(|n| n.parse().ok()).ok_or("the .gih brush count is invalid")?;
    if count == 0 || count > 1000 {
        return Err("the .gih brush count is out of range".into());
    }
    let mut warnings = Vec::new();
    let scatter_pipe = line2.to_ascii_lowercase().contains("sel0:random") && count > 1;
    let mut brushes = Vec::new();
    for i in 0..count {
        let Some(h) = header(bytes, pos) else {
            warnings.push(format!("cell {} is damaged; the rest of the pipe was skipped", i + 1));
            break;
        };
        let tip = tip_of(bytes, &h);
        let fallback = format!("{}-{}", if pipe_name.is_empty() { stem(file_name) } else { pipe_name.clone() }, i + 1);
        let name = if h.name.trim().is_empty() { fallback } else { h.name.clone() };
        let mut b = brush(&h, tip, &name);
        if scatter_pipe {
            b.scatter = 0.3;
        }
        brushes.push(b);
        pos = h.data_start + (h.width as usize * h.height as usize * h.bpp as usize).min(bytes.len().saturating_sub(h.data_start));
    }
    if scatter_pipe && !brushes.is_empty() {
        // A random-cell pipe is one brush that picks a cell per dab: keep its first cell and
        // scatter, instead of turning the cells into a hundred brushes.
        let first = brushes.remove(0);
        warnings.push(format!("the pipe's {} cells are a scatter brush; imported as one brush with 30 % scatter", count));
        brushes.clear();
        brushes.push(first);
    }
    if brushes.is_empty() {
        return Err("the .gih holds no usable cells".into());
    }
    Ok(ParsedFile { brushes, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gbr(w: u32, h: u32, bpp: u32, name: &str, px: &[u8]) -> Vec<u8> {
        let name_bytes = [name.as_bytes(), &[0]].concat();
        let header_size = 28 + name_bytes.len() as u32;
        let mut out = Vec::new();
        for v in [header_size, 2, w, h, bpp, MAGIC, 25] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&name_bytes);
        out.extend_from_slice(px);
        out
    }

    #[test]
    fn reads_a_gbr_and_its_spacing() {
        let mut px = vec![0u8; 9 * 9];
        for y in 2..7 {
            for x in 2..7 {
                px[y * 9 + x] = 255;
            }
        }
        let file = gbr(9, 9, 1, "Speckle", &px);
        assert!(is_gbr(&file));
        let parsed = parse(&file, "Speckle.gbr").unwrap();
        assert_eq!(parsed.brushes.len(), 1);
        assert_eq!(parsed.brushes[0].name, "Speckle");
        assert!((parsed.brushes[0].spacing - 0.25).abs() < 1e-6);
        let tip = parsed.brushes[0].tip.as_ref().unwrap();
        assert_eq!((tip.width, tip.height), (5, 5), "cropped to the painted square");
    }

    #[test]
    fn reads_a_gih_pipe() {
        let cell = |x: u8| {
            let mut px = vec![0u8; 4 * 4];
            px[5] = x;
            gbr(4, 4, 1, "cell", &px)
        };
        let mut file = b"Pipe\n2 1 1 1\n".to_vec();
        let (a, b) = (cell(200), cell(100));
        file.extend_from_slice(&a);
        file.extend_from_slice(&b);
        assert!(is_gih(&file));
        let parsed = parse(&file, "Pipe.gih").unwrap();
        assert_eq!(parsed.brushes.len(), 2);
        assert_eq!(parsed.brushes[1].name, "cell", "each cell keeps the name from its own header");
    }
}

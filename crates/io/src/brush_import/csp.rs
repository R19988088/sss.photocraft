//! CLIP STUDIO PAINT brushes (`.sut`): a SQLite database whose `Variant` table holds the brush
//! parameters, `Node` the name, and `MaterialFile` a tar of the brush's material files (the
//! `data/material.layer` stamp and a PNG preview). Parameters are read column by column; the
//! stamp falls back to the material's PNG preview when its `.layer` cannot be decoded.

use std::collections::BTreeMap;

use super::{ImportedBrush, ImportedDual, ImportedTexture, ParsedFile, tip_from_image_max};

/// Is this a CSP `.sut` (a SQLite database with a brush table)?
pub fn is_sut(bytes: &[u8]) -> bool {
    is_sqlite(bytes)
}

fn is_sqlite(bytes: &[u8]) -> bool {
    bytes.len() >= 100 && bytes.starts_with(b"SQLite format 3\0")
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn varint(b: &[u8], pos: usize) -> Option<(u64, usize)> {
    let mut v: u64 = 0;
    for i in 0..9 {
        let c = *b.get(pos + i)?;
        if i == 8 {
            return Some((v.wrapping_mul(256).wrapping_add(u64::from(c)), pos + 9));
        }
        if c < 0x80 {
            return Some((v.wrapping_mul(128).checked_add(u64::from(c))?, pos + i + 1));
        }
        v = v.wrapping_mul(128).wrapping_add(u64::from(c & 0x7f));
    }
    Some((v, pos + 9))
}

/// A decoded SQLite record value.
#[derive(Clone, Debug)]
pub enum SqlValue {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqlValue {
    fn as_i64(&self) -> Option<i64> {
        match self {
            SqlValue::Int(v) => Some(*v),
            SqlValue::Real(v) => Some(*v as i64),
            _ => None,
        }
    }
    fn as_f64(&self) -> Option<f64> {
        match self {
            SqlValue::Int(v) => Some(*v as f64),
            SqlValue::Real(v) => Some(*v),
            _ => None,
        }
    }
    fn as_blob(&self) -> Option<&[u8]> {
        match self {
            SqlValue::Blob(b) => Some(b),
            _ => None,
        }
    }
    fn as_text(&self) -> Option<&str> {
        match self {
            SqlValue::Text(t) => Some(t),
            _ => None,
        }
    }
}

fn serial_size(t: u64) -> usize {
    match t {
        0 | 8 | 9 => 0,
        1..=4 => t as usize,
        5 => 6,
        6 | 7 => 8,
        t if t >= 12 => ((t - 12) >> 1) as usize,
        _ => 0,
    }
}

fn value_at(b: &[u8], pos: usize, t: u64) -> Option<SqlValue> {
    Some(match t {
        0 => SqlValue::Null,
        8 => SqlValue::Int(0),
        9 => SqlValue::Int(1),
        1..=4 => {
            let n = t as usize;
            let s = b.get(pos..pos + n)?;
            let mut v: i64 = if s[0] & 0x80 != 0 { -1 } else { 0 };
            for x in s {
                v = (v << 8) | i64::from(*x);
            }
            SqlValue::Int(v)
        }
        5 => SqlValue::Int(i64::from(u32_at(b, pos)?) * 4_294_967_296 + i64::from(u32_at(b, pos + 2)?)),
        6 | 7 => {
            let s = b.get(pos..pos + 8)?;
            let mut v = 0u64;
            for x in s {
                v = (v << 8) | u64::from(*x);
            }
            if t == 6 {
                SqlValue::Int((v as i64) << 0)
            } else {
                SqlValue::Real(f64::from_bits(v))
            }
        }
        t if t >= 12 => {
            let len = ((t - 12) >> 1) as usize;
            let data = b.get(pos..pos + len)?.to_vec();
            if t & 1 == 1 { SqlValue::Text(String::from_utf8_lossy(&data).to_string()) } else { SqlValue::Blob(data) }
        }
        _ => SqlValue::Null,
    })
}

fn record(b: &[u8], start: usize, payload_len: usize) -> Option<Vec<SqlValue>> {
    let (hs, mut p) = varint(b, start)?;
    if hs < 1 {
        return None;
    }
    let header_end = start.checked_add(hs as usize)?;
    if header_end > start + payload_len || header_end > b.len() {
        return None;
    }
    let mut types = Vec::new();
    while p < header_end {
        let (t, next) = varint(b, p)?;
        types.push(t);
        p = next;
    }
    let mut vals = Vec::with_capacity(types.len());
    let mut body = header_end;
    for t in types {
        vals.push(value_at(b, body, t)?);
        body = body.checked_add(serial_size(t))?;
    }
    Some(vals)
}

struct Db<'a> {
    bytes: &'a [u8],
    page_size: usize,
    usable: usize,
}

impl<'a> Db<'a> {
    fn open(bytes: &'a [u8]) -> Option<Self> {
        let mut page_size = u16_at(bytes, 16)? as usize;
        if page_size == 0 || page_size == 1 {
            page_size = 65_536;
        }
        if page_size < 512 || page_size > 65_536 || !page_size.is_power_of_two() {
            return None;
        }
        let reserved = usize::from(*bytes.get(20).unwrap_or(&0));
        Some(Db { bytes, page_size, usable: page_size.saturating_sub(reserved) })
    }

    fn payload(&self, cell: usize, total: usize) -> Option<Vec<u8>> {
        if total > self.bytes.len() {
            return None;
        }
        let max_local = self.usable.saturating_sub(35) as usize;
        let min_local = ((self.usable.saturating_sub(12) * 32) / 255).saturating_sub(23);
        let mut local = total;
        let mut overflow = 0u32;
        if total > max_local {
            let mut k = min_local + ((total - min_local) % (self.usable.saturating_sub(4)));
            if k > max_local {
                k = min_local;
            }
            local = k;
            overflow = u32_at(self.bytes, cell + local)?;
        }
        let mut out = vec![0u8; total];
        let head = self.bytes.get(cell..cell + local)?;
        out[..local].copy_from_slice(head);
        let mut filled = local;
        let mut next = overflow;
        let mut guard = 0;
        while next > 0 && filled < total && guard < 100_000 {
            guard += 1;
            let page_off = (next as usize - 1) * self.page_size;
            let hdr = page_off + if next == 1 { 100 } else { 0 };
            let next_page = u32_at(self.bytes, hdr)?;
            let take = (self.usable - 4).min(total - filled);
            let src = self.bytes.get(hdr + 4..hdr + 4 + take)?;
            out[filled..filled + take].copy_from_slice(src);
            filled += take;
            next = next_page;
        }
        Some(out)
    }

    fn rows(&self, root: u32) -> Vec<(i64, Vec<SqlValue>)> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        let mut visited = std::collections::BTreeSet::new();
        while let Some(page) = stack.pop() {
            if page == 0 || !visited.insert(page) || out.len() > 200_000 {
                continue;
            }
            let page_off = (page as usize - 1) * self.page_size;
            let hdr = page_off + if page == 1 { 100 } else { 0 };
            let Some(kind) = self.bytes.get(hdr).copied() else { continue };
            let Some(count) = u16_at(self.bytes, hdr + 3) else { continue };
            let interior = kind == 0x05 || kind == 0x02;
            let ptr_base = hdr + if interior { 12 } else { 8 };
            for i in 0..usize::from(count) {
                let Some(ptr) = u16_at(self.bytes, ptr_base + i * 2).map(usize::from) else { continue };
                let cell = page_off + ptr;
                if interior {
                    if let Some(child) = u32_at(self.bytes, cell) {
                        stack.push(child);
                    }
                    continue;
                }
                let Some((payload_len, p1)) = varint(self.bytes, cell) else { continue };
                let Some((rowid, p2)) = varint(self.bytes, p1) else { continue };
                let Some(payload) = self.payload(p2, payload_len as usize) else { continue };
                if let Some(values) = record(&payload, 0, payload.len()) {
                    out.push((rowid as i64, values));
                }
            }
            if interior && let Some(child) = u32_at(self.bytes, hdr + 8) {
                stack.push(child);
            }
        }
        out
    }
}

struct Table {
    columns: Vec<String>,
    rows: Vec<BTreeMap<String, SqlValue>>,
}

fn tables(db: &Db<'_>) -> BTreeMap<String, Table> {
    let mut out = BTreeMap::new();
    for (_, values) in db.rows(1) {
        let (kind, name, root, sql) = (
            values.first().and_then(SqlValue::as_text).unwrap_or_default(),
            values.get(1).and_then(SqlValue::as_text).unwrap_or_default().to_string(),
            values.get(3).and_then(SqlValue::as_i64).unwrap_or(0) as u32,
            values.get(4).and_then(SqlValue::as_text).unwrap_or_default(),
        );
        if kind != "table" || root == 0 {
            continue;
        }
        let columns = sql.split_once('(').and_then(|(_, rest)| rest.rsplit_once(')')).map_or_else(Vec::new, |(body, _)| {
            body.split(',').map(|c| c.trim().split_whitespace().next().unwrap_or("").trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == '[' || c == ']').to_string()).filter(|c| !c.is_empty()).collect()
        });
        let mut rows = Vec::new();
        for (_, values) in db.rows(root) {
            let mut row = BTreeMap::new();
            for (i, col) in columns.iter().enumerate() {
                if let Some(v) = values.get(i) {
                    row.insert(col.clone(), v.clone());
                }
            }
            rows.push(row);
        }
        out.insert(name, Table { columns, rows });
    }
    out
}

fn num(row: &BTreeMap<String, SqlValue>, key: &str) -> Option<f64> {
    row.get(key).and_then(SqlValue::as_f64)
}

fn flag(row: &BTreeMap<String, SqlValue>, key: &str) -> bool {
    row.get(key).and_then(SqlValue::as_i64).unwrap_or(0) != 0
}

/// Effector blob: flags at offset 8 (0x10 = pressure on), minimum percent at 12.
fn effector(row: &BTreeMap<String, SqlValue>, key: &str) -> (bool, f32) {
    let Some(blob) = row.get(key).and_then(SqlValue::as_blob) else { return (false, 1.0) };
    if blob.len() < 16 {
        return (false, 1.0);
    }
    let flags = u32_at(blob, 8).unwrap_or(0);
    let min = u32_at(blob, 12).unwrap_or(100);
    ((flags & 0x10) != 0, (min as f32 / 100.0).clamp(0.0, 1.0))
}

/// Tar entries of a `MaterialFile` blob: (name, bytes).
fn tar_entries(blob: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 512 <= blob.len() {
        let name_end = blob[p..p + 100].iter().position(|b| *b == 0).unwrap_or(100);
        let name = String::from_utf8_lossy(&blob[p..p + name_end]).to_string();
        if name.is_empty() {
            break;
        }
        let size = blob.get(p + 124..p + 136).and_then(|s| std::str::from_utf8(s).ok()).map(|s| s.trim().parse::<usize>().ok()).flatten().unwrap_or(0);
        let start = p + 512;
        if start + size > blob.len() {
            break;
        }
        out.push((name, blob[start..start + size].to_vec()));
        p = start + size.div_ceil(512) * 512;
    }
    out
}

fn material_tip(blob: &[u8]) -> Option<photocraft_paint::GrayTile> {
    let files = tar_entries(blob);
    // The stamp is data/material.layer; when it cannot be read, the material's PNG preview.
    let png = files.iter().find(|(n, d)| n.to_ascii_lowercase().ends_with(".png") && d.starts_with(b"\x89PNG")).map(|(_, d)| d.clone());
    png.and_then(|b| tip_from_image_max(&b, false).ok())
}

fn model_angle(rotation: f32) -> f32 {
    ((360.0 - rotation) % 360.0 + 360.0) % 360.0
}

fn blend(mode: i64) -> photocraft_paint::MaskMode {
    match mode {
        1 => photocraft_paint::MaskMode::Darken,
        2 => photocraft_paint::MaskMode::Lighten,
        _ => photocraft_paint::MaskMode::Multiply,
    }
}

/// Parse a `.sut`.
pub fn parse(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    if !is_sqlite(bytes) {
        return Err("not a CLIP STUDIO PAINT brush database".into());
    }
    let db = Db::open(bytes).ok_or("the SQLite header is damaged")?;
    let tables = tables(&db);
    let Some(variant) = tables.get("Variant").filter(|t| !t.rows.is_empty()) else {
        return Err("the .sut holds no Variant brush parameters".into());
    };
    let mut out = ParsedFile::default();
    let base_name = tables.get("Node").and_then(|t| t.rows.first()).and_then(|r| r.get("NodeName")).and_then(SqlValue::as_text).map(|s| s.trim_matches('\u{0}').trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| super::stem_of(file_name));
    let materials: Vec<photocraft_paint::GrayTile> = tables
        .get("MaterialFile")
        .map(|t| t.rows.iter().filter_map(|r| r.get("FileData").and_then(SqlValue::as_blob)).filter_map(material_tip).collect())
        .unwrap_or_default();
    if tables.get("MaterialFile").is_some_and(|t| !t.rows.is_empty()) && materials.is_empty() {
        out.warnings.push("the brush material could not be decoded; a round tip stands in".to_string());
    } else if !materials.is_empty() {
        out.warnings.push("CSP tips come from the material preview, not the encrypted stamp layer".to_string());
    }
    let mut seen: Vec<String> = Vec::new();
    for (i, v) in variant.rows.iter().enumerate() {
        if v.get("BrushSize").is_none() && v.get("Opacity").is_none() {
            continue;
        }
        let signature: String = variant.columns.iter().map(|c| format!("{c}={:?}", v.get(c))).collect::<Vec<_>>().join("|");
        if seen.contains(&signature) {
            continue;
        }
        seen.push(signature);
        let size_raw = num(v, "BrushSize").unwrap_or(80.0);
        let size_px = if num(v, "BrushSizeUnit").unwrap_or(0.0) == 1.0 { size_raw * 10.0 } else { size_raw };
        let (size_pressure, size_min) = effector(v, "BrushSizeEffector");
        let (opacity_pressure, opacity_min) = effector(v, "BrushOpacityEffector");
        let tip = if flag(v, "BrushUsePatternImage") { materials.first().cloned() } else { None };
        let texture = if flag(v, "TextureImage") || num(v, "TextureDensity").unwrap_or(0.0) > 0.0 {
            materials.iter().skip(if tip.is_some() { 1 } else { 0 }).next().map(|t| ImportedTexture {
                tile: super::fit_tile(t.clone(), super::MAX_TEXTURE_EDGE),
                depth: (num(v, "TextureDensity").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32,
                scale: (num(v, "TextureScale2").unwrap_or(100.0) / 100.0).clamp(0.01, 10.0) as f32,
                invert: flag(v, "TextureReverseDensity"),
                brightness: 0.0,
                contrast: 0.0,
            })
        } else {
            None
        };
        if flag(v, "BrushUsePatternImage") && tip.is_none() {
            out.warnings.push("the brush pattern image is missing; a round tip stands in".to_string());
        }
        let dual = flag(v, "UseDualBrush").then(|| {
            let unit = num(v, "DualSizeUnit").unwrap_or(0.0);
            let size = num(v, "DualSize").unwrap_or(0.0);
            ImportedDual {
                tip: materials.get(1).cloned(),
                size: (if unit == 1.0 { size * 10.0 } else { size }).clamp(1.0, 5000.0) as f32,
                spacing: (num(v, "DualInterval").unwrap_or(10.0) / 100.0) as f32,
                opacity: (num(v, "DualFlow").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32,
                flow: 1.0,
                hardness: (num(v, "DualHardness").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32,
                roundness: (num(v, "DualThickness").unwrap_or(100.0) / 100.0).clamp(0.01, 1.0) as f32,
                angle: model_angle(num(v, "DualRotation").unwrap_or(0.0) as f32),
                mode: blend(num(v, "DualBrushCompositeMode").unwrap_or(0.0) as i64),
                scatter: 0.0,
                both_axes: true,
                count: 1,
            }
        });
        let spray = flag(v, "BrushUseSpray") && num(v, "BrushSprayDensity").unwrap_or(0.0) > 1.0;
        out.brushes.push(ImportedBrush {
            name: if seen.len() > 1 { format!("{base_name} #{}", i + 1) } else { base_name.clone() },
            size: tip.as_ref().map_or(size_px.clamp(1.0, 4000.0) as f32, |t| t.width.max(t.height) as f32),
            tip,
            spacing: (num(v, "BrushInterval").unwrap_or(10.0) as f32 / 100.0).clamp(0.01, 10.0),
            opacity: (num(v, "Opacity").unwrap_or(100.0) / 100.0).clamp(0.01, 1.0) as f32,
            flow: (num(v, "BrushFlow").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32,
            hardness: (num(v, "BrushHardness").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32,
            angle: model_angle(num(v, "BrushRotation").unwrap_or(0.0) as f32),
            roundness: (num(v, "BrushThickness").unwrap_or(100.0) / 100.0).clamp(0.01, 1.0) as f32,
            pressure_size: size_pressure,
            min_size: size_min,
            pressure_opacity: opacity_pressure,
            min_opacity: opacity_min,
            wet_edges: if flag(v, "BrushUseWaterEdge") { (num(v, "BrushWaterEdgeAlphaPower").unwrap_or(0.0).abs() / 100.0).clamp(0.0, 1.0) as f32 } else { 0.0 },
            smudge: if flag(v, "BrushUseWaterColor") { ((100.0 - num(v, "BrushMixColor").unwrap_or(100.0)) / 100.0).clamp(0.0, 1.0) as f32 } else { 0.0 },
            smoothing: if flag(v, "BrushUseRevision") { (num(v, "BrushRevision").unwrap_or(0.0) / 100.0).clamp(0.0, 1.0) as f32 } else { 0.0 },
            hue_jitter: if flag(v, "BrushChangePatternColor") { (num(v, "BrushHueChange").unwrap_or(0.0).abs() / 360.0).clamp(0.0, 1.0) as f32 } else { 0.0 },
            saturation_jitter: if flag(v, "BrushChangePatternColor") { (num(v, "BrushSaturationChange").unwrap_or(0.0).abs() / 100.0).clamp(0.0, 1.0) as f32 } else { 0.0 },
            brightness_jitter: if flag(v, "BrushChangePatternColor") { (num(v, "BrushValueChange").unwrap_or(0.0).abs() / 100.0).clamp(0.0, 1.0) as f32 } else { 0.0 },
            scatter: if spray { 0.3 } else { 0.0 },
            scatter_count: if spray { (num(v, "BrushSprayDensity").unwrap_or(1.0).round() as i64).clamp(1, 16) as u32 } else { 1 },
            scatter_both_axes: true,
            texture,
            dual,
            taper: (flag(v, "BrushUseIn") || flag(v, "BrushUseOut")).then_some((0.5, 0.6)),
            ..Default::default()
        });
    }
    if out.brushes.is_empty() {
        return Err("the .sut holds no usable brush variants".into());
    }
    Ok(out)
}

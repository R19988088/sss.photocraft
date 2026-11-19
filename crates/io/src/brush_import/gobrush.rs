//! Huawei 天生会画 brushes: a `.gobrush` is a ZIP of `brush_<uuid>.info` (a tag/flag/length
//! record stream), `shape_<uuid>.png` (the tip), `grain_<uuid>.png` (the texture) and a preview.
//! A `.gobrushes` is a ZIP of `.gobrush` files.
//!
//! Settings live in two record blocks inside the info stream: flag `0xC0` holds property
//! records (`[0x10][0xC0][len]` with `0x11` name, `0x18` value, `0x15`/`0x16` bounds), flag
//! `0xB0` holds the numbered parameter block (2 = name, 5 = size, 8 = alpha).

use super::{ImportedBrush, ImportedTexture, ParsedFile, tip_from_image_max};

/// Is this a `.gobrush`/`.gobrushes` (a ZIP of `brush_*.info` + `shape_*.png`)?
pub fn is_gobrush(bytes: &[u8]) -> bool {
    if !super::zip::is_zip(bytes) {
        return false;
    }
    super::zip::names(bytes).iter().any(|n| {
        let l = n.to_ascii_lowercase();
        l.ends_with(".info") || l.ends_with(".gobrush") || l.starts_with("shape_") || l.starts_with("grain_")
    })
}

fn le32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Records of a block: (tag, payload).
fn records(block: &[u8], flag: u8) -> Vec<(u8, Vec<u8>)> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 6 <= block.len() {
        let Some(len) = le32(block, p + 2) else { break };
        let end = p + 6 + len as usize;
        if block[p + 1] != flag || end > block.len() {
            break;
        }
        out.push((block[p], block[p + 6..end].to_vec()));
        p = end;
    }
    out
}

/// The outer record whose tag is 0 and flag is `flag`.
fn outer(info: &[u8], flag: u8) -> Option<Vec<u8>> {
    let mut p = 10;
    while p + 6 <= info.len() {
        let Some(len) = le32(info, p + 2) else { return None };
        let end = p + 6 + len as usize;
        if end > info.len() || ![0xb0, 0xc0, 0xd0].contains(&info[p + 1]) {
            return None;
        }
        if info[p] == 0 && info[p + 1] == flag {
            return Some(info[p + 6..end].to_vec());
        }
        p = end;
    }
    None
}

fn text(payload: &[u8]) -> String {
    String::from_utf8_lossy(payload).trim_end_matches('\u{0}').to_string()
}

/// Property records: name → (value, min, max).
struct Props {
    map: std::collections::BTreeMap<String, (i64, i64, i64)>,
}

fn properties(info: &[u8]) -> Props {
    let mut map = std::collections::BTreeMap::new();
    let Some(block) = outer(info, 0xc0) else { return Props { map } };
    for (_tag, body) in records(&block, 0xc0).into_iter().filter(|(t, _)| *t == 0x10) {
        let inner = records(&body, 0xc0);
        let mut name = String::new();
        let (mut value, mut min, mut max) = (None, 0i64, 0i64);
        for (t, payload) in inner {
            match t {
                0x11 => name = text(&payload),
                0x15 if payload.len() == 4 => max = i64::from(le32(&payload, 0).unwrap_or(0) as i32),
                0x16 if payload.len() == 4 => min = i64::from(le32(&payload, 0).unwrap_or(0) as i32),
                0x18 if payload.len() == 4 => value = Some(i64::from(le32(&payload, 0).unwrap_or(0) as i32)),
                _ => {}
            }
        }
        if !name.is_empty()
            && let Some(v) = value
        {
            map.insert(name, (v, min, max));
        }
    }
    Props { map }
}

impl Props {
    fn get(&self, key: &str) -> Option<i64> {
        self.map.get(key).map(|(v, _, _)| *v)
    }
}

fn params(info: &[u8]) -> std::collections::BTreeMap<u8, i64> {
    let mut out = std::collections::BTreeMap::new();
    let Some(block) = outer(info, 0xb0) else { return out };
    for (tag, payload) in records(&block, 0xb0) {
        if payload.len() == 4 && let Some(v) = le32(&payload, 0) {
            out.insert(tag, i64::from(v as i32));
        }
    }
    out
}

fn brush_name(info: &[u8], fallback: &str) -> String {
    let Some(block) = outer(info, 0xb0) else { return fallback.to_string() };
    for (tag, payload) in records(&block, 0xb0) {
        if tag == 0x02 && (1..=256).contains(&payload.len()) {
            let s = text(&payload);
            if !s.is_empty() && !s.starts_with("$") {
                return s;
            }
        }
    }
    fallback.to_string()
}

fn one(name: &str, info: &[u8], shape: Option<&[u8]>, grain: Option<&[u8]>, warnings: &mut Vec<String>) -> ImportedBrush {
    let props = properties(info);
    let params = params(info);
    let max_size = props.get("propertiesMaxSize").unwrap_or(0);
    let min_size = props.get("propertiesMinSize").unwrap_or(0);
    let slider = params.get(&5).copied().unwrap_or(100);
    let effective = if max_size > 0 { (min_size + (max_size - min_size) * slider / 100).max(1) } else { 100 };
    let tip = shape.and_then(|b| tip_from_image_max(b, false).ok());
    if tip.is_none() {
        warnings.push(format!("{name}: the tip image is missing; a round tip stands in"));
    }
    let alpha = props.get("propertiesMaxAlpha").or_else(|| params.get(&8).copied()).unwrap_or(100);
    let min_alpha = props.get("propertiesMinAlpha").unwrap_or(alpha);
    let depth = props.get("grainDepth").unwrap_or(0);
    let texture = grain.filter(|_| depth > 0).and_then(|b| tip_from_image_max(b, false).ok()).map(|tile| ImportedTexture {
        tile: super::fit_tile(tile, super::MAX_TEXTURE_EDGE),
        depth: (depth as f32 / 100.0).clamp(0.0, 1.0),
        scale: (props.get("grainScale").unwrap_or(100) as f32 / 100.0).clamp(0.01, 10.0),
        invert: false,
        brightness: 0.0,
        contrast: 0.0,
    });
    ImportedBrush {
        name: name.to_string(),
        size: tip.as_ref().map_or(effective as f32, |t| t.width.max(t.height) as f32),
        tip,
        spacing: (props.get("strokeSpacing").unwrap_or(10) as f32 / 100.0).clamp(0.01, 10.0),
        opacity: (alpha as f32 / 100.0).clamp(0.01, 1.0),
        flow: (props.get("renderingFlow").unwrap_or(100) as f32 / 100.0).clamp(0.0, 1.0),
        hardness: 0.8,
        roundness: 1.0,
        pressure_size: min_size > 0 && max_size > min_size,
        min_size: if effective > 0 { (min_size as f32 / effective as f32).clamp(0.0, 1.0) } else { 1.0 },
        pressure_opacity: min_alpha < alpha,
        min_opacity: if alpha > 0 { (min_alpha as f32 / alpha as f32).clamp(0.0, 1.0) } else { 1.0 },
        scatter: (props.get("shapeScatter").unwrap_or(0) as f32 / 100.0).clamp(0.0, 10.0),
        scatter_count: 1,
        scatter_both_axes: true,
        size_jitter: (props.get("strokeJitter").unwrap_or(0) as f32 / 100.0).clamp(0.0, 1.0),
        texture,
        ..Default::default()
    }
}

/// Parse a `.gobrush` or a `.gobrushes` set.
pub fn parse(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    let zip = super::zip::open(bytes)?;
    let mut out = ParsedFile::default();
    let nested: Vec<String> = zip.entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".gobrush")).map(|e| e.name.clone()).collect();
    if !nested.is_empty() {
        for name in nested {
            let Ok(data) = zip.read_name(&name) else { continue };
            if let Ok(mut parsed) = parse(&data, &name) {
                out.brushes.append(&mut parsed.brushes);
                out.warnings.append(&mut parsed.warnings);
            }
        }
        if out.brushes.is_empty() {
            return Err(format!("{file_name}: the set holds no readable brushes"));
        }
        return Ok(out);
    }
    let infos: Vec<String> = zip.entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".info")).map(|e| e.name.clone()).collect();
    if infos.is_empty() {
        return Err(format!("{file_name}: the brush carries no info stream"));
    }
    let shape = zip.find_suffix("shape_").and_then(|e| zip.read(e).ok()).or_else(|| zip.entries.iter().find(|e| e.name.to_ascii_lowercase().starts_with("shape_")).and_then(|e| zip.read(e).ok()));
    let grain = zip.entries.iter().find(|e| e.name.to_ascii_lowercase().starts_with("grain_")).and_then(|e| zip.read(e).ok());
    // "…-副" is the dual sub-brush: import it as its own brush.
    for (i, name) in infos.iter().enumerate() {
        let Ok(data) = zip.read_name(name) else { continue };
        let fallback = if i == 0 { super::stem_of(file_name) } else { format!("{} {}", super::stem_of(file_name), i + 1) };
        let brush = brush_name(&data, &fallback);
        let b = one(&brush, &data, shape.as_deref(), grain.as_deref(), &mut out.warnings);
        out.brushes.push(b);
    }
    Ok(out)
}

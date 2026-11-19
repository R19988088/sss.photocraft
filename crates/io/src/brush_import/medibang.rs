//! MediBang Paint / FireAlpaca materials.
//!
//! * `.mdp` = `mdipack\0` + two reserved u32s + XML length + data length + XML + `PAC` sections.
//!   Each section is a raw RGBA image: either zlib of the whole image or a tile grid of zlib
//!   tiles. The largest layer is the stamp; its alpha channel is the tip.
//! * `Brush2.ini` lists brushes by section (`[0]`, `[1]`…), a bitmap brush naming its tip
//!   (`bitmapFile=…png`) in `brush_bitmap/` beside the ini.

use std::io::Read;

use super::{ImportedBrush, ParsedFile};

/// Is this a `.mdp`?
pub fn is_mdp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"mdipack")
}

/// Is this a `Brush2.ini`?
pub fn is_ini(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    head.trim_start().to_ascii_lowercase().starts_with("[general]") && head.to_ascii_lowercase().contains("activeindex")
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn inflate(b: &[u8]) -> Result<Vec<u8>, ()> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(b).take(256 << 20).read_to_end(&mut out).map_err(|_| ())?;
    Ok(out)
}

struct Pac {
    name: String,
    data: Vec<u8>,
}

fn sections(pac: &[u8]) -> Vec<Pac> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 132 <= pac.len() && &pac[p..p + 4] == b"PAC " {
        let Some(size) = u32_at(pac, p + 4).map(|v| v as usize) else { break };
        if size == 0 || p + size > pac.len() {
            break;
        }
        let name: String = pac[p + 68..p + 132].iter().take_while(|b| **b != 0).map(|b| char::from(*b)).collect();
        out.push(Pac { name, data: pac[p + 132..p + size].to_vec() });
        p += size;
    }
    out
}

/// Decode a `PAC` section into straight RGBA.
fn decode_bin(data: &[u8], width: u32, height: u32) -> Option<(u32, u32, Vec<u8>)> {
    if data.len() < 8 {
        return None;
    }
    let count = u32_at(data, 0)? as usize;
    if (1..=200_000).contains(&count) {
        let mut tiles = Vec::new();
        let mut p = 8;
        let mut tile = 0usize;
        let mut ok = true;
        for _ in 0..count {
            let Some(_col) = u32_at(data, p) else { ok = false; break };
            let Some(_row) = u32_at(data, p + 4) else { ok = false; break };
            let Some(len) = u32_at(data, p + 12).map(|v| v as usize) else { ok = false; break };
            if len == 0 || p + 16 + len > data.len() {
                ok = false;
                break;
            }
            let Ok(raw) = inflate(&data[p + 16..p + 16 + len]) else { ok = false; break };
            if raw.len() % 4 != 0 {
                ok = false;
                break;
            }
            let px = ((raw.len() / 4) as f64).sqrt().round() as usize;
            if px * px * 4 != raw.len() {
                ok = false;
                break;
            }
            if tile == 0 {
                tile = px;
            }
            if px != tile {
                ok = false;
                break;
            }
            tiles.push((u32_at(data, p)?, u32_at(data, p + 4)?, raw));
            p += 16 + len;
            p = (p + 3) & !3;
        }
        if ok && tiles.len() == count && tile > 0 {
            // Tiles are laid out on a grid; reconstruct the canvas.
            let mut canvas_w = width.max(1);
            let mut canvas_h = height.max(1);
            for (col, row, _) in &tiles {
                canvas_w = canvas_w.max((*col + 1) * tile as u32);
                canvas_h = canvas_h.max((*row + 1) * tile as u32);
            }
            let (w, h) = (canvas_w as usize, canvas_h as usize);
            let mut rgba = vec![0u8; w * h * 4];
            for (col, row, raw) in &tiles {
                for y in 0..tile {
                    let dy = *row as usize * tile + y;
                    if dy >= h {
                        break;
                    }
                    let src = y * tile * 4;
                    let dst = (dy * w + *col as usize * tile) * 4;
                    if src + tile * 4 <= raw.len() && dst + tile * 4 <= rgba.len() {
                        rgba[dst..dst + tile * 4].copy_from_slice(&raw[src..src + tile * 4]);
                    }
                }
            }
            return Some((canvas_w, canvas_h, rgba));
        }
    }
    let raw = inflate(data).ok()?;
    let (mut w, mut h) = (width, height);
    if w == 0 || h == 0 || raw.len() != (w as usize * h as usize * 4) {
        let side = ((raw.len() / 4) as f64).sqrt().round() as u32;
        if side > 0 && (side as usize) * (side as usize) * 4 == raw.len() {
            w = side;
            h = side;
        } else {
            return None;
        }
    }
    Some((w, h, raw))
}

fn attr<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let at = xml.find(&key)? + key.len();
    let end = xml[at..].find('"')? + at;
    Some(&xml[at..end])
}

/// Parse a `.mdp` stamp.
fn parse_mdp(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    let xml_len = u32_at(bytes, 12).ok_or("the .mdp header is truncated")? as usize;
    let data_len = u32_at(bytes, 16).ok_or("the .mdp header is truncated")? as usize;
    let xml = bytes.get(20..20 + xml_len).ok_or("the .mdp XML is truncated")?;
    let xml = String::from_utf8_lossy(xml);
    let pac = bytes.get(20 + xml_len..20 + xml_len + data_len).ok_or("the .mdp data block is truncated")?;
    let width = attr(&xml, "width").and_then(|v| v.parse().ok()).unwrap_or(0);
    let height = attr(&xml, "height").and_then(|v| v.parse().ok()).unwrap_or(0);
    let layers: Vec<(String, u32, u32)> = xml
        .match_indices("<Layer ")
        .filter_map(|(at, _)| {
            let tag = &xml[at..];
            let tag = &tag[..tag.find('>')?];
            let bin = attr(tag, "bin")?.to_string();
            let w = attr(tag, "width").and_then(|v| v.parse().ok()).unwrap_or(width);
            let h = attr(tag, "height").and_then(|v| v.parse().ok()).unwrap_or(height);
            Some((bin, w, h))
        })
        .collect();
    let sections = sections(pac);
    let mut best: Option<(u32, u32, Vec<u8>)> = None;
    let mut best_area = 0u64;
    for (bin, w, h) in &layers {
        if let Some(s) = sections.iter().find(|s| &s.name == bin)
            && let Some((iw, ih, px)) = decode_bin(&s.data, *w, *h)
            && u64::from(iw) * u64::from(ih) >= best_area
        {
            best_area = u64::from(iw) * u64::from(ih);
            best = Some((iw, ih, px));
        }
    }
    if best.is_none() {
        for s in &sections {
            if let Some((iw, ih, px)) = decode_bin(&s.data, width, height)
                && u64::from(iw) * u64::from(ih) >= best_area
            {
                best_area = u64::from(iw) * u64::from(ih);
                best = Some((iw, ih, px));
            }
        }
    }
    let Some((w, h, rgba)) = best else {
        return Err(format!("{file_name}: the material holds no readable image"));
    };
    let tip = super::tip_from_rgba_alpha(w, h, &rgba);
    Ok(ParsedFile {
        brushes: vec![ImportedBrush {
            name: super::stem_of(file_name),
            size: tip.width.max(tip.height) as f32,
            spacing: 0.25,
            opacity: 1.0,
            flow: 1.0,
            hardness: 1.0,
            roundness: 1.0,
            tip: Some(tip),
            ..Default::default()
        }],
        warnings: Vec::new(),
    })
}

fn ini_sections(text: &str) -> Vec<std::collections::BTreeMap<String, String>> {
    let mut out = Vec::new();
    let mut cur: Option<std::collections::BTreeMap<String, String>> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some(c) = cur.take() {
                out.push(c);
            }
            cur = section.parse::<u32>().ok().map(|_| std::collections::BTreeMap::new());
            continue;
        }
        if let Some(c) = cur.as_mut()
            && let Some((k, v)) = line.split_once('=')
        {
            c.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    if let Some(c) = cur {
        out.push(c);
    }
    out
}

fn entry_brush(index: usize, e: &std::collections::BTreeMap<String, String>, tip: Option<photocraft_paint::GrayTile>) -> ImportedBrush {
    let width = e.get("width").and_then(|v| v.parse::<f32>().ok()).unwrap_or(10.0).clamp(1.0, 5000.0);
    let min = e.get("min").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0).clamp(0.0, width);
    let opacity = e.get("opacity").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0).clamp(0.0, 1.0);
    let bitmap = e.get("type").is_some_and(|t| t.to_ascii_lowercase().starts_with("bitmap"));
    ImportedBrush {
        name: e.get("name").filter(|n| !n.trim().is_empty()).cloned().unwrap_or_else(|| format!("MediBang {}", index + 1)),
        size: tip.as_ref().map_or(width, |t| t.width.max(t.height) as f32),
        spacing: if bitmap { 1.0 } else { 0.15 },
        opacity: opacity.max(0.01),
        flow: 1.0,
        hardness: 1.0,
        roundness: 1.0,
        pressure_size: e.get("psize").is_none_or(|v| !v.eq_ignore_ascii_case("false")),
        min_size: if width > 0.0 { (min / width).clamp(0.0, 1.0) } else { 0.0 },
        pressure_opacity: e.get("palpha").is_some_and(|v| v.eq_ignore_ascii_case("true")),
        min_opacity: 0.0,
        tip,
        ..Default::default()
    }
}

fn parse_ini_text(text: &str, zip: Option<&super::zip::Archive<'_>>, file_name: &str) -> ParsedFile {
    let mut out = ParsedFile::default();
    for (i, e) in ini_sections(text).into_iter().enumerate() {
        let tip = e.get("bitmapfile").and_then(|name| {
            let zip = zip?;
            let cleaned = name.trim_matches('"');
            zip.find_suffix(cleaned).and_then(|entry| zip.read(entry).ok()).and_then(|b| super::tip_from_image_max(&b, false).ok())
        });
        let brush = entry_brush(i, &e, tip);
        if brush.tip.is_none() && e.contains_key("bitmapfile") {
            out.warnings.push(format!("{}: the brush bitmap is missing; a round tip stands in", brush.name));
        }
        out.brushes.push(brush);
    }
    if out.brushes.is_empty() {
        out.warnings.push(format!("{file_name}: no brush entries were found"));
    }
    out
}

/// Parse a MediBang `.mdp` or `Brush2.ini` (loose or inside a zip).
pub fn parse(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    if is_mdp(bytes) {
        return parse_mdp(bytes, file_name);
    }
    if super::zip::is_zip(bytes) {
        let zip = super::zip::open(bytes)?;
        let Some(ini) = zip.entries.iter().find(|e| e.name.to_ascii_lowercase().ends_with(".ini")).map(|e| e.name.clone()) else {
            return Err(format!("{file_name}: the archive holds no Brush2.ini"));
        };
        let data = zip.read_name(&ini)?;
        let text = String::from_utf8_lossy(&data).to_string();
        let parsed = parse_ini_text(&text, Some(&zip), &ini);
        if parsed.brushes.is_empty() {
            return Err(format!("{file_name}: Brush2.ini lists no brushes"));
        }
        return Ok(parsed);
    }
    if is_ini(bytes) {
        let text = String::from_utf8_lossy(bytes).to_string();
        let parsed = parse_ini_text(&text, None, file_name);
        if parsed.brushes.is_empty() {
            return Err("the ini lists no brushes".into());
        }
        return Ok(parsed);
    }
    Err("not a MediBang material".into())
}

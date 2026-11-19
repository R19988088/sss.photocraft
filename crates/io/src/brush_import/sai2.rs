//! SAI2 brushes: a brush `.ini` (`[Parameters]` with ShapeSize, Spacing, EdgeHardness…) next to
//! a tip image, or a zip "material pack" of them (`brushform/*.conf`). The ini alone carries no
//! tip bitmap, so a pack's `.bmp`/`.png` beside the ini is used when present.

use super::{ImportedBrush, ParsedFile, tip_from_image_max};

/// Is this SAI2 brush `.ini` text?
pub fn is_ini(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    text.contains("[Parameters]")
        && ["ShapeSize", "Spacing", "EdgeHardness", "Scattering", "WxHRatio", "Angle", "Density"].iter().any(|k| text.contains(&format!("{k}=")) || text.contains(&format!("{k} =")))
}

/// Is this a SAI2 material pack (a zip of brush ini/tips)?
pub fn is_pack(bytes: &[u8]) -> bool {
    if !super::zip::is_zip(bytes) {
        return false;
    }
    let names = super::zip::names(bytes);
    names.iter().any(|n| {
        let l = n.to_ascii_lowercase();
        l.ends_with(".conf") || l.ends_with(".ini") || l.contains("brushform") || l.contains("brushtex")
    })
}

fn params(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') || t.starts_with('[') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

fn value<'a>(list: &'a [(String, String)], key: &str) -> Option<&'a str> {
    list.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}

fn number(list: &[(String, String)], key: &str, default: f32) -> f32 {
    value(list, key).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(default)
}

fn roundness_of(list: &[(String, String)]) -> f32 {
    if let Some(text) = value(list, "WxHRatio")
        && let Some((w, h)) = text.split_once(':')
        && let (Ok(w), Ok(h)) = (w.trim().parse::<f32>(), h.trim().parse::<f32>())
        && h > 0.0
    {
        return (w / h).clamp(0.01, 1.0);
    }
    1.0
}

fn brush_of(bytes: &[u8], name: &str, tip_bytes: Option<&[u8]>, warnings: &mut Vec<String>) -> ImportedBrush {
    let text = String::from_utf8_lossy(bytes);
    let list = params(&text);
    let tip = tip_bytes.and_then(|b| tip_from_image_max(b, false).ok());
    if tip.is_none() {
        warnings.push(format!("{name}: the brush ini carries no tip image; a round tip stands in"));
    }
    let shape_size = number(&list, "ShapeSize", 100.0).clamp(1.0, 5000.0);
    let density = number(&list, "Density", f32::NAN);
    let opacity = number(&list, "Opacity", if density.is_finite() { density } else { 100.0 }) / 100.0;
    ImportedBrush {
        name: name.to_string(),
        size: tip.as_ref().map_or(shape_size, |t| t.width.max(t.height) as f32),
        tip,
        spacing: (number(&list, "Spacing", 25.0) / 100.0).clamp(0.01, 10.0),
        opacity: opacity.clamp(0.01, 1.0),
        flow: if density.is_finite() { (density / 100.0).clamp(0.0, 1.0) } else { 1.0 },
        hardness: { let h = number(&list, "EdgeHardness", 100.0); if h > 1.5 { (h / 100.0).clamp(0.0, 1.0) } else { h.clamp(0.0, 1.0) } },
        angle: number(&list, "Angle", 0.0),
        roundness: roundness_of(&list),
        pressure_size: true,
        min_size: 0.2,
        pressure_opacity: false,
        size_jitter: (number(&list, "SizeJitter", 0.0) / 100.0).clamp(0.0, 1.0),
        angle_jitter: (number(&list, "AngleJitter", 0.0) * 3.6 / 360.0).clamp(0.0, 1.0),
        roundness_jitter: (number(&list, "WxHJitter", 0.0) / 100.0).clamp(0.0, 1.0),
        scatter: (number(&list, "Scattering", 0.0) / 100.0).clamp(0.0, 10.0),
        scatter_count: (number(&list, "Count", 1.0).round() as i64).clamp(1, 20) as u32,
        scatter_both_axes: number(&list, "AllDirScattering", 0.0) != 0.0,
        hue_jitter: (number(&list, "HueJitter", 0.0) / 100.0).clamp(0.0, 1.0),
        saturation_jitter: (number(&list, "SaturationJitter", 0.0) / 100.0).clamp(0.0, 1.0),
        brightness_jitter: (number(&list, "BrightnessJitter", 0.0) / 100.0).clamp(0.0, 1.0),
        fg_bg_jitter: (number(&list, "ColJitter", 0.0) / 100.0).clamp(0.0, 1.0),
        ..Default::default()
    }
}

/// Parse a SAI2 `.ini` or a material pack.
pub fn parse(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    if super::zip::is_zip(bytes) {
        return parse_pack(bytes, file_name);
    }
    if !is_ini(bytes) {
        return Err("not a SAI2 brush ini".into());
    }
    let mut warnings = Vec::new();
    let name = super::stem_of(file_name);
    let b = brush_of(bytes, &name, None, &mut warnings);
    Ok(ParsedFile { brushes: vec![b], warnings })
}

fn parse_pack(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    let zip = super::zip::open(bytes)?;
    let mut out = ParsedFile::default();
    // A pack's brush = an ini with a same-stem .bmp/.png beside it.
    let inis: Vec<String> = zip.entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".ini")).map(|e| e.name.clone()).collect();
    for ini in inis {
        let Ok(data) = zip.read_name(&ini) else { continue };
        if !is_ini(&data) {
            continue;
        }
        let stem = ini[..ini.len() - 4].to_string();
        let tip = [".bmp", ".png"].iter().find_map(|ext| zip.find_suffix(&format!("{stem}{ext}")).and_then(|e| zip.read(e).ok()));
        let name = super::stem_of(&ini);
        out.brushes.push(brush_of(&data, &name, tip.as_deref(), &mut out.warnings));
    }
    if out.brushes.is_empty() {
        return Err(format!("{}: the pack holds no brush ini", file_name));
    }
    Ok(out)
}

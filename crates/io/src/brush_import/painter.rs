//! Corel Painter libraries (`.brushlibrary` / `.brushcategory`): a ZIP of
//! `<category>/<brush>.xml` (parameters) and `<brush>.nib` (a JPEG nib). The nib's coverage is
//! the inverse of its luminance against the brightest pixel, which is the backing colour.

use super::{ImportedBrush, ParsedFile};

/// Is this a Painter library (a ZIP with brush XML, or a ZIP holding one)?
pub fn is_library(bytes: &[u8]) -> bool {
    if !super::zip::is_zip(bytes) {
        return false;
    }
    let names = super::zip::names(bytes);
    names.iter().any(|n| n.to_ascii_lowercase().ends_with(".xml"))
        || names.iter().any(|n| {
            let l = n.to_ascii_lowercase();
            l.ends_with(".brushlibrary") || l.ends_with(".brushcategory")
        })
}

fn attr<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let at = xml.find(&key)? + key.len();
    let end = xml[at..].find('"')? + at;
    Some(&xml[at..end])
}

fn num(xml: &str, name: &str, default: f32) -> f32 {
    attr(xml, name).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(default)
}

/// The nib's alpha: `255 * (bg - luma) / bg`, where bg is the image's brightest pixel.
fn nib_tip(bytes: &[u8]) -> Option<photocraft_paint::GrayTile> {
    let image = photocraft_codecs::decode(bytes).ok()?;
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return None;
    }
    let mut luma = vec![0u8; (w * h) as usize];
    let mut bg = 0f32;
    for y in 0..h {
        for x in 0..w {
            let v = if image.layout().channels() >= 3 { (image.get(x, y, 0) + image.get(x, y, 1) + image.get(x, y, 2)) / 3.0 } else { image.get(x, y, 0) };
            luma[(y * w + x) as usize] = (v * 255.0).round() as u8;
            bg = bg.max(v);
        }
    }
    if bg <= 8.0 / 255.0 {
        bg = 220.0 / 255.0;
    }
    let mut cov = vec![0u8; (w * h) as usize];
    for (i, l) in luma.iter().enumerate() {
        cov[i] = (((bg - f32::from(*l) / 255.0) / bg).clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    let crop = super::cropped_tile(w, h, &cov);
    Some(super::fit_tile(crop, super::MAX_TIP_EDGE))
}

fn one(path: &str, xml: &str, tip: Option<photocraft_paint::GrayTile>) -> ImportedBrush {
    let radius = num(xml, "radius", 8.0);
    let min_frac = num(xml, "min-radius-fraction", 1.0);
    let advance = num(xml, "advance", 0.1);
    let thinness = num(xml, "thinness", 1.0);
    let name = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".xml").to_string();
    ImportedBrush {
        name,
        size: tip.as_ref().map_or((radius * 2.0).clamp(2.0, 3000.0), |t| t.width.max(t.height) as f32),
        tip,
        spacing: (advance * 100.0).clamp(1.0, 1000.0) / 100.0,
        opacity: num(xml, "opacity", 1.0).clamp(0.0, 1.0),
        flow: {
            let f = num(xml, "flow", 100.0);
            (if f > 1.0 { f / 400.0 } else { f }).clamp(0.01, 1.0)
        },
        hardness: num(xml, "hardness", 1.0).clamp(0.0, 1.0),
        angle: num(xml, "base-angle", 0.0),
        roundness: (thinness * 100.0).clamp(1.0, 100.0) / 100.0,
        pressure_size: min_frac < 0.99,
        min_size: min_frac.clamp(0.0, 1.0),
        pressure_opacity: false,
        size_jitter: num(xml, "random-size", 0.0).clamp(0.0, 2.0).min(1.0),
        scatter: num(xml, "jitter", 0.0).clamp(0.0, 10.0),
        smoothing: (num(xml, "smoothing", 1.0) / 2.0).clamp(0.0, 1.0),
        wet_edges: if num(xml, "wetness", 0.0) > 0.5 { 1.0 } else { 0.0 },
        ..Default::default()
    }
}

/// Parse a Painter library.
pub fn parse(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    let mut zip_bytes = bytes.to_vec();
    let mut archive = super::zip::open(&zip_bytes)?;
    // A library may be a ZIP that holds the actual `.brushlibrary`.
    if !archive.entries.iter().any(|e| e.name.to_ascii_lowercase().ends_with(".xml"))
        && let Some(inner) = archive.entries.iter().find(|e| {
            let l = e.name.to_ascii_lowercase();
            l.ends_with(".brushlibrary") || l.ends_with(".brushcategory")
        })
    {
        zip_bytes = archive.read(inner)?;
        archive = super::zip::open(&zip_bytes)?;
    }
    let xmls: Vec<String> = archive.entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".xml")).map(|e| e.name.clone()).collect();
    if xmls.is_empty() {
        return Err(format!("{file_name}: the library holds no brush XML"));
    }
    let mut out = ParsedFile::default();
    for path in xmls {
        let Ok(data) = archive.read_name(&path) else { continue };
        let xml = String::from_utf8_lossy(&data);
        let nib = path[..path.len() - 4].to_string() + ".nib";
        let tip = archive.find(&nib).and_then(|e| archive.read(e).ok()).and_then(|b| nib_tip(&b));
        if tip.is_none() {
            out.warnings.push(format!("{path}: the nib image is missing; a round tip stands in"));
        }
        out.brushes.push(one(&path, &xml, tip));
    }
    Ok(out)
}

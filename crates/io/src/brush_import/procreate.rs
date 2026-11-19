//! Procreate brushes: a `.brush` is a ZIP holding `Brush.archive` (a keyed binary plist of
//! parameters) and `Shape.png`; a `.brushset` is a ZIP of one folder per brush plus
//! `brushset.plist` naming and ordering them. `Sub01/` holds the dual tip.

use serde_json::Value;

use super::{ImportedBrush, ImportedDual, ImportedTexture, ParsedFile, tip_from_image_max};

fn number(params: &Value, key: &str, default: f32) -> f32 {
    params.get(key).and_then(Value::as_f64).map_or(default, |v| if v.is_finite() { v as f32 } else { default })
}

fn flag(params: &Value, key: &str) -> bool {
    params.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn name_of(params: &Value, fallback: &str) -> String {
    match params.get("name").and_then(Value::as_str).map(str::trim) {
        Some(n) if !n.is_empty() && n != "$null" => n.to_string(),
        _ => fallback.to_string(),
    }
}

/// Native size interpolation (5.3.8 `lerpAndClampSize`: radius 128, then doubled).
struct SizeRange {
    low: f32,
    high: f32,
    slider: f32,
}

impl SizeRange {
    fn of(params: &Value) -> Self {
        let spacing = number(params, "plotSpacing", 0.0).clamp(0.0, 10.0);
        let limit = 1.0 + 15.0 * (spacing / 0.02).clamp(0.0, 1.0);
        let low = number(params, "minSize", 0.05).clamp(0.0, limit);
        let high = number(params, "maxSize", 1.0).clamp(low, limit);
        let slider = number(params, "paintSize", 0.25).max(0.0);
        SizeRange { low, high, slider }
    }

    fn diameter(&self, factor: f32) -> f32 {
        let radius = (self.high * 128.0).min(self.low.max(128.0 * (self.low + (self.high - self.low) * self.slider * factor.clamp(0.0, 1.0))));
        (radius * 2.0).max(0.0)
    }
}

fn orientation_degrees(params: &Value) -> f32 {
    match params.get("shapeOrientation").and_then(Value::as_f64).unwrap_or(0.0) as i64 {
        2 => 180.0,
        3 => 90.0,
        4 => -90.0,
        _ => 0.0,
    }
}

fn blend(params: &Value) -> photocraft_paint::MaskMode {
    match params.get("dualBlendMode").and_then(Value::as_i64).unwrap_or(0) {
        1 => photocraft_paint::MaskMode::Darken,
        2 => photocraft_paint::MaskMode::Lighten,
        _ => photocraft_paint::MaskMode::Multiply,
    }
}

/// Read one brush folder (`prefix` is "" for a single `.brush`).
fn read_brush(zip: &super::zip::Archive<'_>, prefix: &str, fallback: &str, warnings: &mut Vec<String>) -> Option<ImportedBrush> {
    let archive = zip.find(&format!("{prefix}Brush.archive"))?;
    let bytes = zip.read(archive).ok()?;
    let params = super::plist::parse(&bytes).unwrap_or(Value::Null);
    let range = SizeRange::of(&params);
    let full = range.diameter(1.0);
    let size = full.clamp(1.0, 4096.0);
    let spacing = number(&params, "plotSpacing", 0.0).clamp(0.0, 10.0);
    let press_size = number(&params, "dynamicsPressureSize", 0.0).clamp(0.0, 1.0);
    let press_opacity = number(&params, "dynamicsPressureOpacity", 0.0).clamp(0.0, 1.0);
    let min_opacity = number(&params, "minOpacity", 0.0);
    let max_opacity = number(&params, "maxOpacity", 1.0);
    let paint_opacity = number(&params, "paintOpacity", 1.0);
    let opacity = (min_opacity + (max_opacity - min_opacity) * paint_opacity).clamp(0.01, 1.0);
    let min_size = if press_size > 0.05 && full > 0.0 { (range.diameter(1.0 - press_size) / full).clamp(0.0, 1.0) } else { 1.0 };
    let min_opacity_ratio = if press_opacity > 0.05 { (min_opacity / opacity).clamp(0.0, 1.0) } else { 1.0 };
    let roundness = if full > 0.0 { (range.diameter(number(&params, "shapeRoundness", 1.0)) / full).clamp(0.01, 1.0) } else { 1.0 };
    let angle = number(&params, "shapeAngle", 0.0).to_degrees() + orientation_degrees(&params);
    let inverted = flag(&params, "shapeInverted");
    let shape = zip.find(&format!("{prefix}Shape.png")).and_then(|e| zip.read(e).ok()).and_then(|b| tip_from_image_max(&b, inverted).ok());
    if shape.is_none() {
        let named = params.get("bundledShapePath").and_then(Value::as_str).unwrap_or_default();
        if !named.trim().is_empty() && named != "$null" {
            warnings.push(format!("{}: the tip image ({named}) ships with Procreate, not in the file; a round tip stands in", name_of(&params, fallback)));
        }
    }
    let grain = zip.find(&format!("{prefix}Grain.png")).and_then(|e| zip.read(e).ok()).and_then(|b| tip_from_image_max(&b, flag(&params, "textureInverted")).ok());
    let depth = number(&params, "grainDepth", 0.0).clamp(0.0, 1.0);
    let texture = grain.filter(|_| depth > 0.0).map(|tile| ImportedTexture {
        tile: super::fit_tile(tile, super::MAX_TEXTURE_EDGE),
        depth,
        scale: number(&params, "textureScale", 1.0).max(0.01),
        invert: flag(&params, "textureInverted"),
        brightness: number(&params, "textureBrightness", 0.51) * 2.0 - 1.0,
        contrast: number(&params, "textureContrast", 1.0) - 1.0,
    });
    let dual = read_dual(zip, prefix, warnings);
    let hue = number(&params, "dynamicsPressureHue", 0.0).max(number(&params, "dynamicsJitterHue", 0.0)).max(number(&params, "dynamicsJitterStrokeHue", 0.0));
    let saturation = number(&params, "dynamicsPressureSaturation", 0.0).max(number(&params, "dynamicsJitterSaturation", 0.0));
    let brightness = number(&params, "dynamicsPressureBrightness", 0.0).max(number(&params, "dynamicsJitterLightness", 0.0));
    let taper = number(&params, "taperSize", 0.0) > 0.001 || number(&params, "taperOpacity", 0.0) > 0.001;
    Some(ImportedBrush {
        name: name_of(&params, fallback),
        tip: shape,
        size,
        spacing,
        opacity,
        flow: 1.0,
        hardness: 0.8,
        angle,
        roundness,
        pressure_size: press_size > 0.05,
        min_size,
        pressure_opacity: press_opacity > 0.05,
        min_opacity: min_opacity_ratio,
        size_jitter: number(&params, "dynamicsJitterSize", 0.0).clamp(0.0, 1.0),
        angle_jitter: (number(&params, "shapeScatter", 0.0) * 0.5).clamp(0.0, 1.0),
        scatter: (number(&params, "plotJitter", 0.0) * 0.1).clamp(0.0, 10.0),
        scatter_count: (number(&params, "shapeCount", 0.0) * 15.0).floor().clamp(0.0, 15.0) as u32 + 1,
        scatter_both_axes: true,
        hue_jitter: hue.clamp(0.0, 1.0),
        saturation_jitter: saturation.clamp(0.0, 1.0),
        brightness_jitter: brightness.clamp(0.0, 1.0),
        fg_bg_jitter: number(&params, "jitterSecondary", 0.0).max(number(&params, "jitterStrokeSecondary", 0.0)).clamp(0.0, 1.0),
        wet_edges: number(&params, "wetEdgesAmount", 0.0).max(0.0),
        smoothing: number(&params, "plotSmoothing", 0.0).clamp(0.0, 1.0),
        texture,
        dual,
        taper: taper.then_some((0.5, number(&params, "taperSize", 0.6).clamp(0.0, 1.0))),
        ..Default::default()
    })
}

fn read_dual(zip: &super::zip::Archive<'_>, prefix: &str, warnings: &mut Vec<String>) -> Option<ImportedDual> {
    let sub = zip.find(&format!("{prefix}Sub01/Brush.archive"))?;
    let sub_prefix = format!("{prefix}Sub01/");
    let bytes = zip.read(sub).ok()?;
    let params = super::plist::parse(&bytes).unwrap_or(Value::Null);
    let range = SizeRange::of(&params);
    let full = range.diameter(1.0);
    let tip = zip.find(&format!("{sub_prefix}Shape.png")).and_then(|e| zip.read(e).ok()).and_then(|b| tip_from_image_max(&b, flag(&params, "shapeInverted")).ok());
    if tip.is_none() {
        warnings.push("the dual tip image is missing; a round dual tip stands in".to_string());
    }
    Some(ImportedDual {
        tip,
        size: full.clamp(1.0, 4096.0),
        spacing: number(&params, "plotSpacing", 0.0).clamp(0.0, 10.0),
        opacity: number(&params, "maxOpacity", 1.0).clamp(0.0, 1.0),
        flow: 1.0,
        hardness: 0.8,
        roundness: 1.0,
        angle: number(&params, "shapeAngle", 0.0).to_degrees(),
        mode: blend(&params),
        scatter: 0.0,
        both_axes: true,
        count: 1,
    })
}

fn plist_lists(zip: &super::zip::Archive<'_>) -> (String, Vec<String>) {
    let Some(entry) = zip.find("brushset.plist") else { return (String::new(), Vec::new()) };
    let Ok(bytes) = zip.read(entry) else { return (String::new(), Vec::new()) };
    if super::plist::is_binary(&bytes) {
        let v = super::plist::parse(&bytes).unwrap_or(Value::Null);
        let name = v.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let list = v.get("brushes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        (name, list)
    } else {
        let keys = super::plist::parse_xml_keys(&String::from_utf8_lossy(&bytes));
        (keys.get("name").and_then(|v| v.first().cloned()).unwrap_or_default(), keys.get("brushes").cloned().unwrap_or_default())
    }
}

/// Parse a `.brush` or `.brushset`.
pub fn parse(bytes: &[u8]) -> Result<ParsedFile, String> {
    let zip = super::zip::open(bytes)?;
    if zip.find("Brush.archive").is_some() {
        let mut warnings = Vec::new();
        let Some(b) = read_brush(&zip, "", "Procreate brush", &mut warnings) else {
            return Err("Brush.archive could not be decoded".into());
        };
        return Ok(ParsedFile { brushes: vec![b], warnings });
    }
    let mut names: Vec<String> = zip.entries.iter().filter_map(|e| e.name.split_once('/').map(|(f, _)| f.to_string())).filter(|f| !f.starts_with('.')).collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Err("the brushset holds no brush folders".into());
    }
    let (set_name, ordered) = plist_lists(&zip);
    let set_name = if set_name.trim().is_empty() { "Procreate".to_string() } else { set_name };
    let mut folders: Vec<String> = ordered.into_iter().filter(|f| names.contains(f)).collect();
    for n in &names {
        if !folders.contains(n) {
            folders.push(n.clone());
        }
    }
    let mut out = ParsedFile::default();
    for (i, folder) in folders.iter().enumerate() {
        if let Some(b) = read_brush(&zip, &format!("{folder}/"), &format!("{set_name} {}", i + 1), &mut out.warnings) {
            out.brushes.push(b);
        } else {
            out.warnings.push(format!("{folder}: no Brush.archive"));
        }
    }
    if out.brushes.is_empty() {
        return Err("the brushset holds no readable brushes".into());
    }
    Ok(out)
}

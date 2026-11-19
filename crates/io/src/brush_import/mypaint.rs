//! MyPaint brushes (`.myb`): a JSON document whose `settings` hold base values and pressure
//! response curves. libmypaint adds each input's curve value to the base, so both are read here.

use super::{ImportedBrush, ParsedFile};

/// Is this a MyPaint JSON document?
pub fn is_myb(bytes: &[u8]) -> bool {
    is_myb_text(&String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]))
}

/// The text scan used when a `.myb` has the wrong extension.
pub fn is_myb_text(text: &str) -> bool {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.starts_with('{') && text.contains("\"settings\"") && (text.contains("parent_brush_name") || text.contains("\"version\"") || text.contains("\"comment\""))
}

fn item<'a>(settings: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    settings.get(key)
}

fn num(settings: &serde_json::Value, key: &str) -> Option<f64> {
    match item(settings, key)? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::Object(o) => o.get("base_value").and_then(serde_json::Value::as_f64),
        _ => None,
    }
}

fn points(settings: &serde_json::Value, key: &str) -> Vec<(f64, f64)> {
    let Some(serde_json::Value::Array(c)) = item(settings, key).and_then(|v| v.get("inputs")).and_then(|v| v.get("pressure")) else {
        return Vec::new();
    };
    c.iter()
        .filter_map(|p| {
            let a = p.as_array()?;
            Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
        })
        .collect()
}

/// libmypaint: the pressure curve's value is added to the base value.
fn at_pressure(settings: &serde_json::Value, key: &str, p: f64, default: f64) -> f64 {
    let base = num(settings, key).unwrap_or(default);
    let pts = points(settings, key);
    if pts.len() < 2 {
        return base;
    }
    let mut i = 1;
    while i < pts.len() - 1 && p > pts[i].0 {
        i += 1;
    }
    let (a, b) = (pts[i - 1], pts[i]);
    let delta = if (a.0 - b.0).abs() < f64::EPSILON { a.1 } else { a.1 + (b.1 - a.1) * (p - a.0) / (b.0 - a.0) };
    base + delta
}

fn curve_range(settings: &serde_json::Value, key: &str) -> Option<(f64, f64)> {
    let pts = points(settings, key);
    let lo = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let hi = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    (pts.len() >= 2 && lo.is_finite() && hi.is_finite()).then_some((lo, hi))
}

fn stem(name: &str) -> String {
    std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().trim().to_string()).unwrap_or_default()
}

/// Parse a `.myb` file.
pub fn parse(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    let text = String::from_utf8_lossy(bytes);
    let doc: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("not readable MyPaint JSON: {e}"))?;
    let settings = doc.get("settings").cloned().unwrap_or_else(|| serde_json::json!({}));
    let mut warnings = Vec::new();

    let radius_log = num(&settings, "radius_logarithmic").unwrap_or(1.0);
    let mut size = (radius_log.exp() * 2.0).clamp(1.0, 5000.0) as f32;
    let mut min_size = 1.0;
    let mut pressure_size = false;
    if let Some((lo, hi)) = curve_range(&settings, "radius_logarithmic")
        && hi > lo
    {
        min_size = ((lo.exp() / hi.exp()) as f32).clamp(0.02, 1.0);
        pressure_size = true;
        size = ((radius_log + hi).exp() * 2.0).clamp(1.0, 5000.0) as f32;
    }

    // Opacity is opaque × opaque_multiply along the pen's travel.
    let (mut op_max, mut op_min) = (0.0f64, 1.0f64);
    for step in 0..=20 {
        let p = f64::from(step) / 20.0;
        let v = (at_pressure(&settings, "opaque", p, 1.0).max(0.0) * at_pressure(&settings, "opaque_multiply", p, 1.0)).clamp(0.0, 1.0);
        op_max = op_max.max(v);
        op_min = op_min.min(v);
    }
    let opacity = op_max.clamp(0.01, 1.0) as f32;
    let pressure_opacity = op_max - op_min > 1e-6;
    let min_opacity = if pressure_opacity && op_max > 0.0 { (op_min / op_max) as f32 } else { 1.0 };

    let dabs = num(&settings, "dabs_per_actual_radius").unwrap_or(5.82) + num(&settings, "dabs_per_basic_radius").unwrap_or(0.0);
    let spacing = (1.0 / (2.0 * dabs.max(0.05))) as f32;
    let hardness = num(&settings, "hardness").unwrap_or(0.5) as f32;
    let ratio = num(&settings, "elliptical_dab_ratio").unwrap_or(1.0);
    let angle = num(&settings, "elliptical_dab_angle").unwrap_or(90.0) as f32;
    let roundness = (1.0 / ratio.max(1.0)) as f32;
    let smudge = num(&settings, "smudge").unwrap_or(0.0) as f32;
    let eraser = num(&settings, "eraser").unwrap_or(0.0) > 0.0;
    let scatter = num(&settings, "offset_by_random").unwrap_or(0.0).max(0.0) as f32;
    let size_jitter = num(&settings, "radius_by_random").unwrap_or(0.0).max(0.0) as f32;
    let hue_jitter = num(&settings, "change_color_h").unwrap_or(0.0).abs() as f32;
    let saturation_jitter =
        num(&settings, "change_color_hsl_s").unwrap_or(0.0).abs().max(num(&settings, "change_color_hsv_s").unwrap_or(0.0).abs()) as f32;
    let brightness_jitter =
        num(&settings, "change_color_v").unwrap_or(0.0).abs().max(num(&settings, "change_color_l").unwrap_or(0.0).abs()) as f32;
    let smoothing = (num(&settings, "slow_tracking").unwrap_or(0.0).max(0.0) / 2.0) as f32;
    if num(&settings, "dabs_per_second").unwrap_or(0.0) > 0.0 {
        warnings.push("the brush sprays dabs over time; imported with its static spacing".to_string());
    }
    if eraser {
        warnings.push("the brush is an eraser; imported as an erasing preset".to_string());
    }

    let parent = doc.get("parent_brush_name").and_then(serde_json::Value::as_str).unwrap_or_default();
    let name = match parent.strip_prefix("brushconv/") {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ if !stem(file_name).is_empty() => stem(file_name),
        _ if !parent.trim().is_empty() => parent.trim().to_string(),
        _ => "MyPaint Brush".to_string(),
    };

    Ok(ParsedFile {
        brushes: vec![ImportedBrush {
            name,
            size,
            spacing,
            opacity,
            flow: 1.0,
            hardness,
            angle,
            roundness,
            pressure_size,
            min_size,
            pressure_opacity,
            min_opacity,
            size_jitter,
            scatter,
            hue_jitter,
            saturation_jitter,
            brightness_jitter,
            smudge,
            smoothing,
            eraser,
            ..Default::default()
        }],
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_myb_with_pressure_size() {
        let json = r#"{
            "version": 3, "parent_brush_name": "brushconv/Ink Pen",
            "settings": {
                "radius_logarithmic": {"base_value": 1.0, "inputs": {"pressure": [[0, -1.6], [1, 0.7]]}},
                "hardness": 0.8,
                "opaque": {"base_value": 1.0, "inputs": {"pressure": [[0, 0], [1, 1]]}},
                "opaque_multiply": 1.0,
                "dabs_per_actual_radius": 4.0
            }
        }"#;
        assert!(is_myb(json.as_bytes()));
        let parsed = parse(json.as_bytes(), "Ink.myb").unwrap();
        let b = &parsed.brushes[0];
        assert_eq!(b.name, "Ink Pen");
        assert!(b.pressure_size, "{b:?}");
        assert!((b.hardness - 0.8).abs() < 1e-6);
        assert!((b.spacing - 0.125).abs() < 1e-6, "{}", b.spacing);
    }
}

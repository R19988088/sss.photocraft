//! Krita presets: a `.kpp` is a PNG whose `tEXt`/`iTXt` chunk `preset` carries the preset XML
//! (a `<Preset>` of `<param>` entries, with `brush_definition` holding the `<Brush>` element).
//! A `.bundle` is a ZIP of such presets plus the resources they name.

use super::{ImportedBrush, ImportedTexture, ParsedFile, tip_from_image};

/// Is this a Krita `.kpp` (a PNG carrying preset XML)?
pub fn is_kpp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n") && text_chunks(bytes).is_some_and(|t| t.iter().any(|(k, v)| k == "preset" && v.contains("<Preset")))
}

/// PNG text chunks: (`tEXt`/`iTXt` keyword, text), inflating `zTXt`/compressed `iTXt`.
fn text_chunks(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut out = Vec::new();
    let mut p = 8;
    while p + 12 <= bytes.len() {
        let len = u32::from_be_bytes(bytes.get(p..p + 4)?.try_into().ok()?) as usize;
        let kind = bytes.get(p + 4..p + 8)?;
        let data = bytes.get(p + 8..p + 8 + len)?;
        match kind {
            b"tEXt" => {
                if let Some((k, v)) = latin1_pair(data) {
                    out.push((k, v));
                }
            }
            b"zTXt" => {
                let (k, rest) = latin1_pair(data)?;
                let body = rest.strip_prefix('\0')?.as_bytes();
                if let Ok(text) = inflate(body) {
                    out.push((k, String::from_utf8_lossy(&text).to_string()));
                }
            }
            b"iTXt" => {
                if let Some((k, v)) = itxt(data) {
                    out.push((k, v));
                }
            }
            _ => {}
        }
        p += 12 + len;
    }
    Some(out)
}

fn latin1_pair(data: &[u8]) -> Option<(String, String)> {
    let zero = data.iter().position(|b| *b == 0)?;
    let key = String::from_utf8_lossy(&data[..zero]).to_string();
    let value: String = data[zero + 1..].iter().map(|b| char::from(*b)).collect();
    Some((key, value))
}

fn itxt(data: &[u8]) -> Option<(String, String)> {
    let zero = data.iter().position(|b| *b == 0)?;
    let key = String::from_utf8_lossy(&data[..zero]).to_string();
    let compressed = *data.get(zero + 1)? != 0;
    let mut q = zero + 3;
    let lang = data.get(q..)?.iter().position(|b| *b == 0)?;
    q += lang + 1;
    let translated = data.get(q..)?.iter().position(|b| *b == 0)?;
    q += translated + 1;
    let text = data.get(q..)?;
    if compressed {
        return Some((key, String::from_utf8_lossy(&inflate(text).ok()?).to_string()));
    }
    Some((key, String::from_utf8_lossy(text).to_string()))
}

fn inflate(bytes: &[u8]) -> Result<Vec<u8>, ()> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(bytes).take(8 << 20).read_to_end(&mut out).map_err(|_| ())?;
    Ok(out)
}

/// `<param name="…" type="…">value</param>` entries, flat text scan.
fn params(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find("<param") {
        let tail = &rest[at..];
        let Some(close) = tail.find("</param>") else { break };
        let head = tail.find('>').map(|i| &tail[..i]).unwrap_or_default();
        let name = attr(head, "name").unwrap_or_default();
        let body = &tail[tail.find('>').map_or(0, |i| i + 1)..close];
        let value = body.replace("<![CDATA[", "").replace("]]>", "");
        if !name.is_empty() {
            out.push((name.to_string(), value.trim().to_string()));
        }
        rest = &tail[close + 8..];
    }
    out
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let at = tag.find(&key)? + key.len();
    let end = tag[at..].find('"')? + at;
    Some(&tag[at..end])
}

fn param<'a>(list: &'a [(String, String)], name: &str) -> Option<&'a str> {
    list.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// The numbers in a param's text, in order.
fn numbers(list: &[(String, String)], name: &str) -> Vec<f32> {
    param(list, name)
        .map(|t| {
            t.split(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '.')))
                .filter_map(|n| n.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn boolean(list: &[(String, String)], name: &str) -> Option<bool> {
    param(list, name).map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
}

fn b64(s: &str) -> Option<Vec<u8>> {
    photocraft_paint::tile::b64_decode(s.trim())
}

/// Parse a `.kpp` or a `.bundle`.
pub fn parse(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    if super::zip::is_zip(bytes) {
        return parse_bundle(bytes, file_name);
    }
    let parsed = parse_kpp(bytes, file_name)?;
    if parsed.brushes.is_empty() {
        return Err("the Krita preset holds no brush".into());
    }
    Ok(parsed)
}

fn parse_bundle(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    let archive = super::zip::open(bytes)?;
    let names: Vec<String> = archive.entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".kpp")).map(|e| e.name.clone()).collect();
    if names.is_empty() {
        return Err("the bundle holds no .kpp presets".into());
    }
    let mut out = ParsedFile::default();
    for name in names {
        let Ok(data) = archive.read_name(&name) else { continue };
        match parse_kpp(&data, &name) {
            Ok(mut parsed) => {
                out.brushes.append(&mut parsed.brushes);
                out.warnings.append(&mut parsed.warnings);
            }
            Err(e) => out.warnings.push(format!("{name}: {e}")),
        }
    }
    if out.brushes.is_empty() {
        return Err(format!("{}: the bundle's presets could not be read", file_name));
    }
    Ok(out)
}

fn parse_kpp(bytes: &[u8], file_name: &str) -> Result<ParsedFile, String> {
    let chunks = text_chunks(bytes).ok_or("not a PNG preset")?;
    let xml = chunks.iter().find(|(k, _)| k == "preset").map(|(_, v)| v.as_str()).ok_or("the PNG has no preset chunk")?;
    if !xml.contains("<Preset") {
        return Err("the preset chunk holds no <Preset>".into());
    }
    let mut warnings = Vec::new();
    let list = params(xml);
    let name = attr(xml.split("<Preset").nth(1).and_then(|s| s.split('>').next()).unwrap_or_default(), "name").map(str::to_string).unwrap_or_default();
    let definition = param(&list, "brush_definition").unwrap_or_default();

    let mut size = attr(definition, "diameter").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    let mut spacing = attr(definition, "spacing").and_then(|v| v.parse::<f32>().ok()).map(|v| v).unwrap_or(f32::NAN);
    let angle = attr(definition, "angle").and_then(|v| v.parse::<f32>().ok()).map(|v| v.to_degrees()).unwrap_or(0.0);
    let roundness = attr(definition, "ratio").and_then(|v| v.parse::<f32>().ok()).map(|v| v.clamp(0.01, 1.0)).unwrap_or(1.0);
    let fade = attr(definition, "hfade").and_then(|v| v.parse::<f32>().ok());
    let hardness = fade.map(|f| (1.0 - f).clamp(0.0, 1.0)).unwrap_or(1.0);
    if !size.is_finite() || size <= 0.0 {
        size = numbers(&list, "brush_radius").first().map(|r| *r * 2.0).or_else(|| numbers(&list, "diameter").first().copied()).unwrap_or(40.0);
    }
    if !spacing.is_finite() {
        spacing = numbers(&list, "spacing").first().copied().unwrap_or(0.25);
        if spacing > 1.5 {
            spacing /= 100.0;
        }
    }
    let opacity = {
        let v = numbers(&list, "OpacityValue").first().copied().or_else(|| numbers(&list, "opacity").first().copied()).unwrap_or(1.0);
        if v > 1.5 { v / 100.0 } else { v }
    };
    let flow = {
        let v = numbers(&list, "FlowValue").first().copied().or_else(|| numbers(&list, "flow").first().copied()).unwrap_or(1.0);
        if v > 1.5 { v / 100.0 } else { v }
    };
    let size_dynamic = boolean(&list, "PressureSize").unwrap_or(false)
        || boolean(&list, "SizeUseCurve").unwrap_or(false) && param(&list, "SizeSensor").is_some_and(|s| s.contains("pressure"));
    let opacity_dynamic = boolean(&list, "PressureOpacity").unwrap_or(false)
        || boolean(&list, "OpacityUseCurve").unwrap_or(false) && param(&list, "OpacitySensor").is_some_and(|s| s.contains("pressure"));
    let min_size = numbers(&list, "SizeValue").first().copied().filter(|v| *v > 0.0 && *v <= 1.0).unwrap_or(0.2);
    let scatter = numbers(&list, "ScatterValue").first().copied().unwrap_or(0.0).max(0.0);
    let scatter_on = boolean(&list, "Scatter/Enabled").unwrap_or(scatter > 0.001);
    let scatter_count = numbers(&list, "Scatter/Count").first().map_or(1, |v| (*v).round().clamp(1.0, 16.0) as u32);
    let both_axes = boolean(&list, "Scatter/Y").unwrap_or(true);
    let smudge = numbers(&list, "ColorRateValue").first().copied().unwrap_or(0.0).clamp(0.0, 1.0);
    let texture_on = boolean(&list, "Texture/Pattern/Enabled").unwrap_or(false);
    let texture = if texture_on { texture_of(&list, &mut warnings) } else { None };

    let tip = tip_of(xml).or_else(|| tip_from_image(bytes, false).ok());    if tip.is_none() {
        warnings.push("the preset embeds no tip; a round tip stands in".to_string());
    }
    if attr(definition, "filename").is_some() {
        warnings.push("the preset names an external brush resource; its embedded copy was used".to_string());
    }
    let name = if name.trim().is_empty() { super::stem_of(file_name) } else { name };
    Ok(ParsedFile {
        brushes: vec![ImportedBrush {
            name,
            size: tip.as_ref().map_or(size, |t| t.width.max(t.height) as f32),
            spacing,
            opacity,
            flow,
            hardness,
            angle,
            roundness,
            pressure_size: size_dynamic,
            min_size,
            pressure_opacity: opacity_dynamic,
            min_opacity: if opacity_dynamic { 0.3 } else { 1.0 },
            scatter: if scatter_on { scatter } else { 0.0 },
            scatter_count: if scatter_on { scatter_count } else { 1 },
            scatter_both_axes: both_axes,
            smudge,
            texture,
            tip,
            ..Default::default()
        }],
        warnings,
    })
}

fn texture_of(list: &[(String, String)], warnings: &mut Vec<String>) -> Option<ImportedTexture> {
    let embedded = param(list, "Texture/Pattern/Pattern").and_then(b64);
    let tile = embedded.and_then(|outer| b64(&String::from_utf8_lossy(&outer))).and_then(|png| tip_from_image(&png, false).ok());
    if tile.is_none() {
        warnings.push("the texture is enabled but its image is missing; imported without it".to_string());
        return None;
    }
    Some(ImportedTexture {
        tile: super::fit_tile(tile?, super::MAX_TEXTURE_EDGE),
        depth: numbers(list, "Texture/Strength/Value").first().copied().unwrap_or(0.5).clamp(0.0, 1.0),
        scale: numbers(list, "Texture/Pattern/Scale").first().copied().unwrap_or(1.0).max(0.01),
        invert: boolean(list, "Texture/Pattern/Invert").unwrap_or(false),
        brightness: 0.0,
        contrast: 0.0,
    })
}

fn tip_of(xml: &str) -> Option<photocraft_paint::GrayTile> {
    let mut rest = xml;
    while let Some(at) = rest.find("<resource") {
        let tail = &rest[at..];
        let head = tail.find('>').map(|i| &tail[..i]).unwrap_or_default();
        let Some(end) = tail.find("</resource>") else { break };
        if attr(head, "type").is_some_and(|t| t.to_ascii_lowercase().contains("brush")) {
            let body_start = tail.find('>').map_or(0, |i| i + 1);
            if let Some(data) = b64(&tail[body_start..end].replace("<![CDATA[", "").replace("]]>", "")) {
                if let Ok(t) = tip_from_image(&data, false) {
                    return Some(t);
                }
                if let Some(t) = super::gimp::parse(&data, "tip").ok().and_then(|p| p.brushes.into_iter().next().and_then(|b| b.tip)) {
                    return Some(t);
                }
            }
        }
        rest = &tail[end + 11..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_with_text(key: &str, text: &str) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&crc32fast::hash(&[kind, data].concat()).to_be_bytes());
            out
        };
        png.extend_from_slice(&chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]));
        let mut body = key.as_bytes().to_vec();
        body.push(0);
        body.extend_from_slice(text.as_bytes());
        png.extend_from_slice(&chunk(b"tEXt", &body));
        png
    }

    #[test]
    fn reads_preset_params_and_definition() {
        let xml = r#"<Preset name="Pencil" paintopid="paintbrush"><param name="brush_definition" type="string"><![CDATA[<Brush spacing="0.15" angle="0.785"><MaskGenerator type="circle" diameter="40" ratio="0.5" hfade="0.2"/></Brush>]]></param><param name="OpacityValue" type="float">87</param><param name="PressureSize" type="bool">true</param><param name="ScatterValue" type="float">0.1</param></Preset>"#;
        let file = png_with_text("preset", xml);
        assert!(is_kpp(&file));
        let parsed = parse(&file, "Pencil.kpp").unwrap();
        let b = &parsed.brushes[0];
        assert_eq!(b.name, "Pencil");
        assert!((b.size - 40.0).abs() < 1e-6, "{}", b.size);
        assert!((b.spacing - 0.15).abs() < 1e-6, "{}", b.spacing);
        assert!((b.opacity - 0.87).abs() < 1e-6, "{}", b.opacity);
        assert!((b.angle - 45.0).abs() < 1.0, "{}", b.angle);
        assert!((b.roundness - 0.5).abs() < 1e-6);
        assert!(b.pressure_size);
        assert!((b.scatter - 0.1).abs() < 1e-6);
    }
}

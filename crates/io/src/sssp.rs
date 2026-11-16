//! SSSP import: C3's own project format (see `docs/custom/SSSP.md`) into a document.
//!
//! C3 keeps a document as a SQLite container (`crates/sssp` reads it): metadata JSON with one
//! entry per layer, and pixels as RGBA16Float tiles. This module turns that into a
//! [`Document`]:
//!
//! * raster layers come back as `RGBA32F` surfaces assembled from their tiles;
//! * a 网点 (`kind: "halftone"`) adjustment maps onto [`Adjustment::Halftone`];
//! * every other adjustment kind keeps its JSON in [`Adjustment::Unsupported`], so a round trip
//!   does not lose the stack C3 built (PSD cannot hold those kinds at all);
//! * groups are imported as a warning-free flat list for now (C3's group nesting is recorded
//!   separately in its metadata and is not needed to view the artwork).

use photocraft_color::{BlendMode, ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Adjustment, Document, Layer, LayerContent};
use photocraft_geom::{Rect, Size};
use photocraft_raster::Surface;
use serde_json::Value;

use crate::{ExportResult, ImportResult, IoError};

type Result<T> = std::result::Result<T, IoError>;

fn invalid(message: impl Into<String>) -> IoError {
    IoError::Sssp(message.into())
}

/// `""` / `"0"`-tolerant field readers for C3's JSON.
fn number(layer: &Value, key: &str) -> Option<f64> {
    layer.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

fn boolean(layer: &Value, key: &str, fallback: bool) -> bool {
    layer.get(key).and_then(Value::as_bool).unwrap_or(fallback)
}

fn text<'a>(layer: &'a Value, key: &str) -> Option<&'a str> {
    layer.get(key).and_then(Value::as_str)
}

/// C3 writes blend names as camelCase (`softLight`); our enum's labels are `Soft Light`.
fn blend_mode(name: Option<&str>) -> BlendMode {
    let Some(name) = name else { return BlendMode::Normal };
    let key: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    match key.as_str() {
        "passthrough" => BlendMode::PassThrough,
        "normal" => BlendMode::Normal,
        "dissolve" => BlendMode::Dissolve,
        "darken" => BlendMode::Darken,
        "multiply" => BlendMode::Multiply,
        "colorburn" => BlendMode::ColorBurn,
        "linearburn" => BlendMode::LinearBurn,
        "darkercolor" => BlendMode::DarkerColor,
        "lighten" => BlendMode::Lighten,
        "screen" => BlendMode::Screen,
        "colordodge" => BlendMode::ColorDodge,
        "lineardodge" | "add" => BlendMode::LinearDodge,
        "lightercolor" => BlendMode::LighterColor,
        "overlay" => BlendMode::Overlay,
        "softlight" => BlendMode::SoftLight,
        "hardlight" => BlendMode::HardLight,
        "vividlight" => BlendMode::VividLight,
        "linearlight" => BlendMode::LinearLight,
        "pinlight" => BlendMode::PinLight,
        "hardmix" => BlendMode::HardMix,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        "subtract" => BlendMode::Subtract,
        "divide" => BlendMode::Divide,
        "hue" => BlendMode::Hue,
        "saturation" => BlendMode::Saturation,
        "color" => BlendMode::Color,
        "luminosity" => BlendMode::Luminosity,
        _ => BlendMode::Normal,
    }
}

/// C3 stores `imageDataBase64` and tiles; either can carry a raster layer's pixels.
fn raster_pixels(project: &photocraft_sssp::Project, layer: &Value) -> Result<Vec<u8>> {
    let id = text(layer, "id").ok_or_else(|| invalid("layer without id"))?;
    let tiles = project.tiles_of(id);
    if tiles.is_empty() {
        // An empty layer: C3 writes no tiles at all for a layer with nothing on it.
        return Ok(vec![0; project.canvas_width() as usize * project.canvas_height() as usize * 8]);
    }
    photocraft_sssp::assemble_layer(project.canvas_width(), project.canvas_height(), &tiles)
        .map_err(|e| invalid(format!("layer {id} tiles: {e}")))
}

/// RGBA half floats → the `f32` samples a surface holds.
fn to_f32_samples(raw: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(raw.len() / 2);
    for pair in raw.chunks_exact(2) {
        let bits = u16::from_le_bytes([pair[0], pair[1]]);
        out.push(photocraft_codecs::f16::from_bits(bits).to_f32());
    }
    out
}

/// `kind: "psHueSaturation"` (C3's own import of the PSD block) → our own adjustment.
fn hue_saturation(adjustment: &Value) -> Option<Adjustment> {
    let payload = adjustment.get("payload")?.get("psHueSaturation")?;
    Some(Adjustment::HueSaturation {
        hue: number(payload, "hue").unwrap_or(0.0) as f32,
        saturation: number(payload, "saturation").unwrap_or(0.0) as f32,
        lightness: number(payload, "lightness").unwrap_or(0.0) as f32,
        colorize: boolean(payload, "colorize", false),
        ranges: photocraft_doc::adjust::HueRange::defaults(),
    })
}

fn hex_color(text: &str) -> Option<[f32; 3]> {
    let hex = text.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let mut out = [0.0; 3];
    for (i, channel) in out.iter_mut().enumerate() {
        let byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        *channel = byte as f32 / 255.0;
    }
    Some(out)
}

/// `kind: "gradientMap"` with C3's payload → our gradient map (stops and reverse; C3's
/// intensity / mix mode / tonal bias have no equivalent and stay in the recorded stack).
fn gradient_map(adjustment: &Value) -> Option<Adjustment> {
    let payload = adjustment.get("payload")?.get("gradientMap")?;
    let stops = payload.get("stops")?.as_array()?;
    let mut map: Vec<(f32, [f32; 3])> = stops
        .iter()
        .filter_map(|stop| {
            let at = number(stop, "position")? as f32;
            let color = hex_color(text(stop, "color")?)?;
            Some((at, color))
        })
        .collect();
    if map.len() < 2 {
        return None;
    }
    map.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(Adjustment::GradientMap {
        stops: map,
        reverse: boolean(payload, "reverse", false),
        dither: false,
    })
}

fn line_art_blend(name: Option<&str>) -> photocraft_doc::adjust::LineArtBlend {
    use photocraft_doc::adjust::LineArtBlend;
    match name {
        Some("multiply") => LineArtBlend::Multiply,
        Some("screen") => LineArtBlend::Screen,
        Some("overlay") => LineArtBlend::Overlay,
        _ => LineArtBlend::Normal,
    }
}

/// `kind: "cmykScreenPrint"` (C3's separate CMYK layer): its settings sit flat in the payload.
fn cmyk_screen_print(adjustment: &Value) -> Option<Adjustment> {
    let payload = adjustment.get("payload")?;
    Some(Adjustment::CmykScreenPrint {
        dot_size: number(payload, "cmykDotSize").unwrap_or(82.0) as f32,
        angle: number(payload, "cmykAngle").unwrap_or(15.0) as f32,
        cyan: number(payload, "cmykCyan").unwrap_or(0.0) as f32,
        magenta: number(payload, "cmykMagenta").unwrap_or(0.0) as f32,
        yellow: number(payload, "cmykYellow").unwrap_or(0.0) as f32,
        black: number(payload, "cmykBlack").unwrap_or(0.0) as f32,
    })
}

/// `kind: "lineArt"` (线条强化): C3 keeps its own object in the payload.
fn line_art(adjustment: &Value) -> Option<Adjustment> {
    let payload = adjustment.get("payload")?.get("lineArt")?;
    Some(Adjustment::LineArt {
        intensity: number(payload, "intensity").unwrap_or(100.0) as f32,
        sharpness: number(payload, "sharpness").unwrap_or(100.0) as f32,
        thickness: number(payload, "thickness").unwrap_or(13.0) as f32,
        shadows: number(payload, "shadowsOpacity").unwrap_or(24.0) as f32,
        midtones: number(payload, "midtonesOpacity").unwrap_or(32.0) as f32,
        highlights: number(payload, "highlightsOpacity").unwrap_or(90.0) as f32,
        color: text(payload, "color").and_then(hex_color).unwrap_or([0.0, 0.0, 0.0]),
        blend: line_art_blend(text(payload, "blendMode")),
    })
}

/// `kind: "halftone"` with C3's payload → our halftone adjustment.
///
/// C3 measures the screen in lines (`frequency`, 1..96) and renders cells of `180 / frequency`
/// pixels (its shader clamps at 2 px), angles in degrees, `density` / `contrast` / `brightness` /
/// `jitter` in percent, dots per channel only when `colorMode == "color"`. `levels`, `smooth`,
/// `toneUniformity`, `randomness` and the character shapes (`shape: "custom"`) have no field here
/// and stay in the layer's recorded stack.
fn halftone(adjustment: &Value) -> Option<Adjustment> {
    let payload = adjustment.get("payload")?.get("halftone")?;
    let frequency = number(payload, "frequency").unwrap_or(32.0) as f32;
    // C3's shader: `cellSize = max(2.0, 180.0 / frequency)`.
    let size = (180.0 / frequency.max(1.0)).max(2.0);
    let shape = match text(payload, "shape").unwrap_or("circle") {
        "square" => photocraft_doc::adjust::HalftoneShape::Square,
        "line" => photocraft_doc::adjust::HalftoneShape::Line,
        "ellipse" => photocraft_doc::adjust::HalftoneShape::Ellipse,
        _ => photocraft_doc::adjust::HalftoneShape::Round,
    };
    let color = match text(payload, "colorMode") {
        Some("color") => true,
        Some(_) => false,
        None => boolean(payload, "color", false),
    };
    Some(Adjustment::Halftone {
        size,
        angle: number(payload, "angle").unwrap_or(45.0) as f32,
        density: number(payload, "density").unwrap_or(50.0) as f32,
        contrast: number(payload, "contrast").unwrap_or(0.0) as f32,
        brightness: number(payload, "brightness").unwrap_or(0.0) as f32,
        jitter: number(payload, "jitter").unwrap_or(0.0) as f32,
        shape,
        color,
        preserve_white: boolean(payload, "preserveWhite", true),
        white: number(payload, "white").unwrap_or(0.0) as f32,
        black: number(payload, "black").unwrap_or(0.0) as f32,
    })
}

/// Namespace under which a layer keeps the whole C3 adjustment stack.
///
/// C3 merges every kind of an adjustment layer into **one** stack (`enabledKinds`,
/// `strengths`, `processingOrder`, one payload per kind); a document here keeps one adjustment
/// per layer and one panel per adjustment, so the stack has no place of its own. The original
/// stack is recorded on the layer as XMP (PSD keeps layer XMP too), so the order and the kinds
/// this build cannot evaluate survive a save — and C3 gets them back on export.
pub const SSPP_ADJUSTMENT_XMP: &str = "sssp.adjustment";

fn record_stack(layer: &mut Layer, layer_json: &Value) {
    let Some(adjustment) = layer_json.get("adjustment") else { return };
    let packet = serde_json::json!({ SSPP_ADJUSTMENT_XMP: adjustment });
    if let Ok(text) = serde_json::to_string(&packet) {
        layer.xmp = Some(std::sync::Arc::from(text.as_str()));
    }
}

/// The recorded C3 stack, when this layer came from an `.sssp` project.
fn recorded_stack(layer: &Layer) -> Option<Value> {
    let text = layer.xmp.as_deref()?;
    let packet: Value = serde_json::from_str(text).ok()?;
    packet.get(SSPP_ADJUSTMENT_XMP).cloned()
}

fn adjustment_layer(layer: &Value, warnings: &mut Vec<String>) -> LayerContent {
    let name = text(layer, "name").unwrap_or("Adjustment");
    let Some(adjustment) = layer.get("adjustment") else {
        warnings.push(format!("{name}: an adjustment layer carries no adjustment; kept as an empty group"));
        return LayerContent::Adjustment(Adjustment::Unsupported {
            psd_key: "adj?".into(),
            raw: std::sync::Arc::from(Vec::new()),
        });
    };
    let kind = text(adjustment, "kind").unwrap_or("");
    let enabled: Vec<&str> = adjustment
        .get("enabledKinds")
        .and_then(Value::as_array)
        .map(|kinds| kinds.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let order: Vec<&str> = adjustment
        .get("processingOrder")
        .and_then(Value::as_array)
        .map(|kinds| kinds.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    // A merged stack shows its own `kind` first; when this build has no panel for it, the first
    // kind of the stack it does understand takes over (the rest stays in the recorded stack).
    let candidates: Vec<&str> = std::iter::once(kind)
        .chain(order.iter().copied())
        .chain(enabled.iter().copied())
        .filter(|k| !k.is_empty())
        .collect();
    for candidate in candidates {
        let mapped = match candidate {
            "halftone" => halftone(adjustment),
            "psHueSaturation" => hue_saturation(adjustment),
            "gradientMap" => gradient_map(adjustment),
            "cmykScreenPrint" => cmyk_screen_print(adjustment),
            "lineArt" => line_art(adjustment),
            _ => None,
        };
        if let Some(adjustment) = mapped {
            if enabled.len() > 1 && !enabled.iter().all(|k| *k == candidate) {
                warnings.push(format!(
                    "{name}: C3 merged {} effects into one panel; this build shows {candidate} and keeps the rest in the layer's SSSP record",
                    enabled.len()
                ));
            }
            return LayerContent::Adjustment(adjustment);
        }
    }
    let raw = serde_json::to_vec(adjustment).unwrap_or_default();
    warnings.push(format!("{name}: the {kind} adjustment has no equivalent here and is kept as imported"));
    LayerContent::Adjustment(Adjustment::Unsupported {
        psd_key: if kind.is_empty() { "adj?".to_string() } else { kind.to_string() },
        raw: std::sync::Arc::from(raw),
    })
}

/// Import an `.sssp` project.
pub fn import(name: &str, bytes: &[u8]) -> Result<ImportResult> {
    let project = photocraft_sssp::read_bytes(bytes).map_err(|e| invalid(format!("{e}")))?;
    let width = project.canvas_width();
    let height = project.canvas_height();
    if width == 0 || height == 0 {
        return Err(invalid("the project has an empty canvas"));
    }
    let mut warnings = Vec::new();
    let mut document = Document::new(
        name,
        Size::new(width, height),
        ColorMode::Rgb,
        SampleType::F32,
    );
    let dpi = project
        .metadata
        .get("canvasDPI")
        .and_then(Value::as_f64)
        .unwrap_or(72.0) as f32;
    document.resolution_dpi = dpi;
    if project.metadata.get("groups").and_then(Value::as_array).is_some_and(|g| !g.is_empty()) {
        warnings.push("group nesting is not imported yet: the layers are placed flat".into());
    }

    // C3 lists layers top to bottom; a document stores them bottom to top.
    for layer in project.layers().iter().rev() {
        let name = text(layer, "name").unwrap_or("Layer").to_string();
        let kind = text(layer, "type").unwrap_or("raster");
        let content = match kind {
            "adjustment" => adjustment_layer(layer, &mut warnings),
            _ => {
                let pixels = raster_pixels(&project, layer)?;
                let samples = to_f32_samples(&pixels);
                let mut surface = Surface::new(PixelFormat::RGBA32F);
                surface
                    .try_write_region(
                        Rect { x0: 0, y0: 0, x1: width as i32, y1: height as i32 },
                        &samples,
                    )
                    .map_err(|e| invalid(format!("{name}: {e}")))?;
                LayerContent::Raster(surface)
            }
        };
        let mut out = Layer::new(name, content);
        out.visible = boolean(layer, "visible", true);
        out.blend = blend_mode(text(layer, "blendMode"));
        out.opacity = number(layer, "opacity").unwrap_or(1.0) as f32;
        if boolean(layer, "locked", false) {
            out.locks = photocraft_doc::Locks { all: true, ..Default::default() };
        }
        if matches!(out.content, LayerContent::Adjustment(_)) {
            record_stack(&mut out, layer);
        }
        document.layers.push(out);
    }
    Ok(ImportResult { document, warnings, source_read_only: true, preview_only: false })
}

/// C3's camelCase name for a blend mode (the inverse of [`blend_mode`]).
fn blend_text(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::PassThrough => "passThrough",
        BlendMode::Normal => "normal",
        BlendMode::Dissolve => "dissolve",
        BlendMode::Darken => "darken",
        BlendMode::Multiply => "multiply",
        BlendMode::ColorBurn => "colorBurn",
        BlendMode::LinearBurn => "linearBurn",
        BlendMode::DarkerColor => "darkerColor",
        BlendMode::Lighten => "lighten",
        BlendMode::Screen => "screen",
        BlendMode::ColorDodge => "colorDodge",
        BlendMode::LinearDodge => "linearDodge",
        BlendMode::LighterColor => "lighterColor",
        BlendMode::Overlay => "overlay",
        BlendMode::SoftLight => "softLight",
        BlendMode::HardLight => "hardLight",
        BlendMode::VividLight => "vividLight",
        BlendMode::LinearLight => "linearLight",
        BlendMode::PinLight => "pinLight",
        BlendMode::HardMix => "hardMix",
        BlendMode::Difference => "difference",
        BlendMode::Exclusion => "exclusion",
        BlendMode::Subtract => "subtract",
        BlendMode::Divide => "divide",
        BlendMode::Hue => "hue",
        BlendMode::Saturation => "saturation",
        BlendMode::Color => "color",
        BlendMode::Luminosity => "luminosity",
        _ => "normal",
    }
}

fn halftone_json(adjustment: &Adjustment) -> Option<Value> {
    let Adjustment::Halftone {
        size,
        angle,
        density,
        contrast,
        brightness,
        jitter,
        shape,
        color,
        preserve_white,
        white,
        black,
    } = adjustment
    else {
        return None;
    };
    let shape = match shape {
        photocraft_doc::adjust::HalftoneShape::Square => "square",
        photocraft_doc::adjust::HalftoneShape::Line => "line",
        photocraft_doc::adjust::HalftoneShape::Ellipse => "ellipse",
        photocraft_doc::adjust::HalftoneShape::Round => "round",
        _ => "round",
    };
    Some(serde_json::json!({
        "kind": "halftone",
        "payload": { "halftone": {
            // C3 measures the screen as lines: its shader uses cells of 180 / frequency pixels.
            "frequency": (180.0 / size.max(0.5)).clamp(1.0, 96.0),
            "angle": angle, "density": density, "contrast": contrast,
            "brightness": brightness, "jitter": jitter, "shape": shape,
            "colorMode": if *color { "color" } else { "mono" },
            "preserveWhite": preserve_white, "white": white, "black": black,
        }},
        "enabledKinds": ["halftone"],
        "strengths": {},
        "processingOrder": ["halftone"],
    }))
}

/// Save a document as an SSSP project C3 can open again.
pub fn export(doc: &Document, ctl: &photocraft_raster::Interrupt) -> Result<ExportResult> {
    let width = doc.size.width;
    let height = doc.size.height;
    let mut warnings = Vec::new();
    let project = photocraft_sssp::Project::new_document(width, height, 256);
    let tile_size = project.tile_size();
    let rect = Rect { x0: 0, y0: 0, x1: width as i32, y1: height as i32 };
    let mut layers = Vec::new();
    let mut tiles = Vec::new();
    // A document lists layers bottom to top; C3 stores them top to bottom.
    for (index, layer) in doc.layers.iter().rev().enumerate() {
        ctl.check().map_err(|_| IoError::Cancelled)?;
        let id = format!("pc-{index}");
        let mut json = serde_json::json!({
            "id": id,
            "name": layer.name,
            "visible": layer.visible,
            "locked": layer.locks.all,
            "isSolidBackground": false,
            "opacity": layer.opacity,
            "blendMode": blend_text(layer.blend),
            "clipsToLayerBelow": layer.clipped,
            "colorHex": "#FFFFFF",
            "style": {},
        });
        match &layer.content {
            LayerContent::Raster(surface) => {
                let samples = surface
                    .try_read_region(rect)
                    .map_err(|e| invalid(format!("{}: {e}", layer.name)))?;
                let mut half = Vec::with_capacity(samples.len() * 2);
                for sample in &samples {
                    half.extend_from_slice(&photocraft_codecs::f16::from_f32(*sample).to_bits().to_le_bytes());
                }
                match photocraft_sssp::split_layer(&id, width, height, tile_size, &half, &id) {
                    Ok(mut layer_tiles) => tiles.append(&mut layer_tiles),
                    Err(e) => {
                        warnings.push(format!("{}: tiles were left empty ({e})", layer.name));
                    }
                }
                json["type"] = Value::String("raster".into());
                json["imageDataBase64"] = Value::String(String::new());
            }
            LayerContent::Adjustment(adjustment) => {
                json["type"] = Value::String("adjustment".into());
                // A layer that came from C3 carries its whole stack (kind order and the kinds
                // nothing here evaluates): write that back so the merged panel stays merged on
                // C3's side, and only refresh the part this build really shows.
                if let Some(mut recorded) = recorded_stack(layer) {
                    // Refresh the one kind this build really shows, keep the rest of the stack.
                    if let Some(halftone) = halftone_json(adjustment) {
                        recorded["kind"] = Value::String("halftone".into());
                        recorded["payload"]["halftone"] = halftone["payload"]["halftone"].clone();
                    } else if let Adjustment::CmykScreenPrint { dot_size, angle, cyan, magenta, yellow, black } = adjustment {
                        let payload = &mut recorded["payload"];
                        payload["cmykDotSize"] = Value::from(*dot_size);
                        payload["cmykAngle"] = Value::from(*angle);
                        payload["cmykCyan"] = Value::from(*cyan);
                        payload["cmykMagenta"] = Value::from(*magenta);
                        payload["cmykYellow"] = Value::from(*yellow);
                        payload["cmykBlack"] = Value::from(*black);
                        payload["cmykMode"] = Value::Bool(true);
                    } else if let Adjustment::LineArt { intensity, sharpness, thickness, shadows, midtones, highlights, color, blend } = adjustment {
                        recorded["payload"]["lineArt"] = serde_json::json!({
                            "blendMode": match blend {
                                photocraft_doc::adjust::LineArtBlend::Normal => "normal",
                                photocraft_doc::adjust::LineArtBlend::Multiply => "multiply",
                                photocraft_doc::adjust::LineArtBlend::Screen => "screen",
                                photocraft_doc::adjust::LineArtBlend::Overlay => "overlay",
                            },
                            "color": format!(
                                "#{:02X}{:02X}{:02X}",
                                (color[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                                (color[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                                (color[2] * 255.0).round().clamp(0.0, 255.0) as u8
                            ),
                            "intensity": intensity, "sharpness": sharpness, "thickness": thickness,
                            "shadowsOpacity": shadows, "midtonesOpacity": midtones, "highlightsOpacity": highlights,
                        });
                    } else if let Adjustment::GradientMap { stops, reverse, .. } = adjustment {
                        recorded["payload"]["gradientMap"]["reverse"] = Value::Bool(*reverse);
                        let mut stops_json: Vec<Value> = stops
                            .iter()
                            .map(|(at, color)| {
                                serde_json::json!({
                                    "position": at,
                                    "color": format!(
                                        "#{:02X}{:02X}{:02X}",
                                        (color[0] * 255.0).round() as u8,
                                        (color[1] * 255.0).round() as u8,
                                        (color[2] * 255.0).round() as u8
                                    ),
                                })
                            })
                            .collect();
                        recorded["payload"]["gradientMap"]["stops"] = Value::Array(std::mem::take(&mut stops_json));
                    } else if let Adjustment::HueSaturation { hue, saturation, lightness, colorize, .. } = adjustment {
                        recorded["payload"]["psHueSaturation"] = serde_json::json!({
                            "colorize": colorize, "hue": hue, "saturation": saturation, "lightness": lightness,
                        });
                    }
                    json["adjustment"] = recorded;
                    layers.push(json);
                    continue;
                }
                match halftone_json(adjustment) {
                    Some(adjustment) => json["adjustment"] = adjustment,
                    None => match adjustment {
                        Adjustment::Unsupported { psd_key, raw } => {
                            match serde_json::from_slice::<Value>(raw) {
                                Ok(original) => json["adjustment"] = original,
                                Err(_) => {
                                    warnings.push(format!(
                                        "{}: the {psd_key} adjustment has no SSSP payload and is saved empty",
                                        layer.name
                                    ));
                                    json["adjustment"] = serde_json::json!({
                                        "kind": psd_key, "payload": {}, "enabledKinds": [psd_key],
                                        "strengths": {}, "processingOrder": [psd_key],
                                    });
                                }
                            }
                        }
                        _ => {
                            warnings.push(format!(
                                "{}: {} cannot be expressed in SSSP yet and is saved empty",
                                layer.name,
                                adjustment.label()
                            ));
                            json["adjustment"] = serde_json::json!({
                                "kind": "unknown", "payload": {}, "enabledKinds": [],
                                "strengths": {}, "processingOrder": [],
                            });
                        }
                    },
                }
            }
            other => {
                warnings.push(format!("{}: {} layers are flattened away in SSSP", layer.name, other.kind_name()));
                json["type"] = Value::String("raster".into());
            }
        }
        layers.push(json);
    }
    let ids: Vec<Value> = layers.iter().map(|l| l["id"].clone()).collect();
    let mut project = project;
    project.metadata["layers"] = Value::Array(layers);
    project.metadata["layerOrder"] = Value::Array(ids);
    project.metadata["canvasDPI"] = Value::from(doc.resolution_dpi);
    project.tiles = tiles;
    let bytes = photocraft_sssp::write_bytes(&project).map_err(|e| invalid(format!("{e}")))?;
    ctl.progress(1.0);
    Ok(ExportResult { bytes, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Option<Vec<u8>> {
        let path = std::path::PathBuf::from(std::env::var("HOME").ok()?).join("Desktop/小 p 设计/s2.sssp");
        path.is_file().then(|| std::fs::read(&path).ok()).flatten()
    }

    #[test]
    fn imports_a_real_c3_project() {
        let Some(bytes) = sample() else {
            eprintln!("skipped: no .sssp sample on this machine");
            return;
        };
        let result = import("s2.sssp", &bytes).expect("import");
        assert_eq!(result.document.size.width, 5000);
        assert_eq!(result.document.size.height, 6000);
        assert_eq!(result.document.layers.len(), 8);
        // The sample's adjustment layer is C3's own `psHueSaturation`, which maps onto ours.
        assert!(result.document.layers.iter().any(|layer| matches!(
            &layer.content,
            LayerContent::Adjustment(Adjustment::HueSaturation { .. })
        )), "the imported adjustment layer keeps working: {:?}", result.warnings);
        // A raster layer really carries pixels: the bottom one is the background.
        let raster = result.document.layers.iter().find_map(|layer| match &layer.content {
            LayerContent::Raster(surface) => Some(surface),
            _ => None,
        }).expect("a raster layer");
        assert!(raster.tiles().count() > 0);
        println!("warnings: {:?}", result.warnings);
    }

    fn sample_gradient_map() -> Option<Vec<u8>> {
        let path = std::path::PathBuf::from(std::env::var("HOME").ok()?).join("Desktop/小 p 目录/sp__20261102131541.sssp");
        path.is_file().then(|| std::fs::read(&path).ok()).flatten()
    }

    #[test]
    fn a_gradient_map_layer_keeps_its_stops() {
        let Some(bytes) = sample_gradient_map() else {
            eprintln!("skipped: no gradient map sample on this machine");
            return;
        };
        let result = import("sp.sssp", &bytes).expect("import");
        let map = result.document.layers.iter().find_map(|layer| match &layer.content {
            LayerContent::Adjustment(Adjustment::GradientMap { stops, reverse, .. }) => Some((stops.clone(), *reverse)),
            _ => None,
        });
        let Some((stops, reverse)) = map else {
            panic!("the gradient map layer did not import: {:?}", result.warnings);
        };
        assert_eq!(stops.len(), 4);
        assert!(stops.windows(2).all(|w| w[0].0 <= w[1].0), "stops stay in order");
        assert!(!reverse);
        // Round trip: the stack record keeps the kinds C3 wrote.
        let ctl = photocraft_raster::Interrupt::new(&|| false, &|_| {});
        let saved = export(&result.document, &ctl).expect("export");
        let project = photocraft_sssp::read_bytes(&saved.bytes).expect("re-read");
        let stack = project.layers().iter().find_map(|layer| layer.get("adjustment")).expect("a stack");
        assert_eq!(stack.get("kind").and_then(Value::as_str), Some("gradientMap"));
        assert!(stack["payload"]["gradientMap"]["stops"].as_array().is_some_and(|s| s.len() >= 2));
    }

    /// A merged C3 panel: one layer, two effects. This build shows the first one it understands and
    /// keeps the whole stack recorded, so C3 gets both back.
    #[test]
    fn cmyk_and_line_art_kinds_map_to_their_own_layers() {
        let mut project = photocraft_sssp::Project::new_document(20, 20, 16);
        project.metadata["layers"] = serde_json::json!([
            {
                "id": "cmyk", "name": "CMYK", "type": "adjustment", "visible": true, "opacity": 1.0, "blendMode": "normal",
                "adjustment": {
                    "kind": "cmykScreenPrint", "enabledKinds": ["cmykScreenPrint"], "strengths": {},
                    "payload": { "cmykDotSize": 60, "cmykAngle": 22, "cmykMode": true,
                                 "cmykCyan": 12, "cmykMagenta": -8, "cmykYellow": 4, "cmykBlack": 33 }
                }
            },
            {
                "id": "line", "name": "线条强化", "type": "adjustment", "visible": true, "opacity": 1.0, "blendMode": "normal",
                "adjustment": {
                    "kind": "lineArt", "enabledKinds": ["lineArt"], "strengths": {},
                    "payload": { "lineArt": { "blendMode": "multiply", "color": "#112233", "highlightsOpacity": 90,
                        "intensity": 100, "midtonesOpacity": 32, "shadowsOpacity": 24, "sharpness": 100, "thickness": 13 } }
                }
            }
        ]);
        let bytes = photocraft_sssp::write_bytes(&project).expect("write");
        let result = import("kinds.sssp", &bytes).expect("import");
        let cmyk = result.document.layers.iter().find_map(|l| match &l.content {
            LayerContent::Adjustment(Adjustment::CmykScreenPrint { dot_size, angle, cyan, magenta, yellow, black }) => {
                Some((*dot_size, *angle, *cyan, *magenta, *yellow, *black))
            }
            _ => None,
        }).expect("a CMYK layer");
        assert_eq!(cmyk, (60.0, 22.0, 12.0, -8.0, 4.0, 33.0));
        let line = result.document.layers.iter().find_map(|l| match &l.content {
            LayerContent::Adjustment(Adjustment::LineArt { thickness, blend, color, .. }) => Some((*thickness, *blend, *color)),
            _ => None,
        }).expect("a line art layer");
        assert_eq!(line.0, 13.0);
        assert_eq!(line.1, photocraft_doc::adjust::LineArtBlend::Multiply);
        assert!((line.2[0] - 0x11 as f32 / 255.0).abs() < 0.01);

        // And both go back to C3 under their own kinds.
        let ctl = photocraft_raster::Interrupt::new(&|| false, &|_| {});
        let saved = export(&result.document, &ctl).expect("export");
        let project = photocraft_sssp::read_bytes(&saved.bytes).expect("re-read");
        let kinds: Vec<String> = project.layers().iter().filter_map(|l| l.get("adjustment")).map(|a| a["kind"].as_str().unwrap_or("").to_string()).collect();
        assert_eq!(kinds, ["cmykScreenPrint", "lineArt"]);
        let cmyk_back = &project.layers()[0]["adjustment"]["payload"];
        assert_eq!(cmyk_back["cmykDotSize"], serde_json::json!(60.0));
        assert_eq!(cmyk_back["cmykBlack"], serde_json::json!(33.0));
        let line_back = &project.layers()[1]["adjustment"]["payload"]["lineArt"];
        assert_eq!(line_back["blendMode"].as_str(), Some("multiply"));
        assert_eq!(line_back["thickness"], serde_json::json!(13.0));
    }

    #[test]
    fn halftone_frequency_becomes_the_screen_period() {
        let mut project = photocraft_sssp::Project::new_document(20, 20, 16);
        project.metadata["layers"] = serde_json::json!([{
            "id": "dots", "name": "网点", "type": "adjustment", "visible": true, "opacity": 1.0, "blendMode": "normal",
            "adjustment": {
                "kind": "halftone", "enabledKinds": ["halftone"], "strengths": {},
                "payload": { "halftone": { "frequency": 20, "angle": 45, "density": 88, "contrast": 4,
                    "brightness": -22, "jitter": 8, "shape": "circle", "colorMode": "mono", "preserveWhite": true } }
            }
        }]);
        let bytes = photocraft_sssp::write_bytes(&project).expect("write");
        let result = import("dots.sssp", &bytes).expect("import");
        let halftone = result.document.layers.iter().find_map(|l| match &l.content {
            LayerContent::Adjustment(Adjustment::Halftone { size, angle, density, color, .. }) => Some((*size, *angle, *density, *color)),
            _ => None,
        }).expect("a halftone layer");
        // C3's shader: cell = 180 / frequency, clamped at 2 px.
        assert!((halftone.0 - 9.0).abs() < 0.01, "{halftone:?}");
        assert_eq!(halftone.1, 45.0);
        assert_eq!(halftone.2, 88.0);
        assert!(!halftone.3, "mono stays mono");
    }

    /// Renders a real project through the flat exporter, as a picture to look at. Run with
    /// `cargo test -p photocraft-io --lib sssp::tests::renders_a_real_project -- --ignored`.
    #[test]
    #[ignore]
    fn renders_a_real_project() {
        let Some(bytes) = sample() else { return };
        let result = import("s2.sssp", &bytes).expect("import");
        let ctl = photocraft_raster::Interrupt::new(&|| false, &|_| {});
        let exported = crate::export_with(&result.document, "sssp-render.png", &crate::ExportOptions::default(), &ctl)
            .expect("render");
        std::fs::write("/tmp/sssp-render.png", &exported.bytes).expect("write");
        eprintln!("rendered {} bytes, warnings {:?}", exported.bytes.len(), exported.warnings);
    }

    #[test]
    fn a_merged_stack_keeps_every_kind_and_its_order() {
        let mut project = photocraft_sssp::Project::new_document(40, 30, 16);
        project.metadata["layers"] = serde_json::json!([{
            "id": "merged",
            "name": "合并调整",
            "type": "adjustment",
            "visible": true,
            "opacity": 1.0,
            "blendMode": "normal",
            "adjustment": {
                "kind": "screenPrint",
                "enabledKinds": ["halftone", "gradientMap"],
                "processingOrder": ["gradientMap", "halftone"],
                "strengths": { "gradientMap": 1.0, "halftone": 0.5 },
                "payload": {
                    "halftone": { "size": 9, "angle": 30, "density": 55, "shape": "round" },
                    "gradientMap": {
                        "reverse": false,
                        "stops": [
                            { "position": 0, "color": "#13293d" },
                            { "position": 1, "color": "#f6b26b" }
                        ]
                    }
                }
            }
        }]);
        let bytes = photocraft_sssp::write_bytes(&project).expect("write");
        let result = import("merged.sssp", &bytes).expect("import");
        assert_eq!(result.document.layers.len(), 1);
        // `kind` (screenPrint) has no panel here, so the first kind of the order takes over.
        match &result.document.layers[0].content {
            LayerContent::Adjustment(Adjustment::GradientMap { stops, .. }) => assert_eq!(stops.len(), 2),
            other => panic!("expected the gradient map, got {other:?}: {:?}", result.warnings),
        }
        assert!(result.warnings.iter().any(|w| w.contains("merged 2 effects")), "{:?}", result.warnings);

        let ctl = photocraft_raster::Interrupt::new(&|| false, &|_| {});
        let saved = export(&result.document, &ctl).expect("export");
        let project = photocraft_sssp::read_bytes(&saved.bytes).expect("re-read");
        let stack = &project.layers()[0]["adjustment"];
        assert_eq!(stack["kind"].as_str(), Some("screenPrint"), "the layer keeps C3's own kind");
        assert_eq!(stack["enabledKinds"], serde_json::json!(["halftone", "gradientMap"]));
        assert_eq!(stack["processingOrder"], serde_json::json!(["gradientMap", "halftone"]));
        assert_eq!(stack["strengths"]["halftone"], serde_json::json!(0.5));
        assert!(stack["payload"]["halftone"]["size"].as_f64().is_some(), "the other kind's payload stays");
    }

    #[test]
    fn a_real_project_round_trips_through_the_document() {
        let Some(bytes) = sample() else {
            eprintln!("skipped: no .sssp sample on this machine");
            return;
        };
        let result = import("s2.sssp", &bytes).expect("import");
        let ctl = photocraft_raster::Interrupt::new(&|| false, &|_| {});
        let saved = export(&result.document, &ctl).expect("export");
        let project = photocraft_sssp::read_bytes(&saved.bytes).expect("re-read");
        assert_eq!(project.canvas_width(), 5000);
        assert_eq!(project.canvas_height(), 6000);
        assert_eq!(project.layers().len(), 8);
        // C3 opens it again with its own reader: the same heads and a manual generation.
        assert_eq!(project.head, photocraft_sssp::Head::Manual);
        // And our own reader gets the same picture back.
        let again = import("again.sssp", &saved.bytes).expect("re-import");
        assert_eq!(again.document.layers.len(), 8);
        // The merged C3 stack (order and the kinds nothing here evaluates) is written back verbatim.
        let original = photocraft_sssp::read_bytes(&bytes).expect("original");
        for (before, after) in original.layers().iter().zip(project.layers().iter()) {
            if let Some(stack) = before.get("adjustment") {
                assert_eq!(
                    after.get("adjustment").and_then(|a| a.get("processingOrder")),
                    stack.get("processingOrder"),
                    "{} keeps its stack order",
                    before["name"]
                );
                assert_eq!(
                    after.get("adjustment").and_then(|a| a.get("enabledKinds")),
                    stack.get("enabledKinds"),
                    "{} keeps its enabled kinds",
                    before["name"]
                );
            }
        }
        println!("warnings: {:?}", saved.warnings);
    }
}

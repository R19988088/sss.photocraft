//! Brushes from other painting apps, mapped onto PhotoCraft brush presets.
//!
//! One entry point, [`read_any`], sniffs the file and hands it to the parser for its format:
//! Photoshop `.abr`/`.tpl`, GIMP `.gbr`/`.gih`, MyPaint `.myb`, Krita `.kpp`/`.bundle`,
//! Procreate `.brush`/`.brushset`, SAI2 brush packs and `.ini`, Huawei `.gobrush`/`.gobrushes`,
//! MediBang `.mdp`/`.ini`, Corel Painter libraries and CLIP STUDIO PAINT `.sut`.
//!
//! Parsers decode the file into [`ImportedBrush`] — a neutral description of a brush (tip
//! bitmap, size, spacing, dynamics, scatter, texture, dual tip…) — and [`ImportedBrush::preset`]
//! maps it onto [`BrushSettings`] once, so every format follows the same rules. Settings a
//! format carries but PhotoCraft has no equivalent for are listed in the import's `warnings`
//! instead of being dropped silently.

use photocraft_paint::{
    BrushPreset, BrushSettings, Control, DualBrush, Dynamic, GrayTile, MaskMode, Pattern, Scattering, ShapeDynamics, TipShape, Transfer,
};

pub mod csp;
pub mod gimp;
pub mod gobrush;
pub mod krita;
pub mod medibang;
pub mod mypaint;
pub mod painter;
pub mod plist;
pub mod procreate;
pub mod sai2;
pub mod tpl;
pub mod zip;

/// Largest decoded tip edge kept as is; larger tips are box-downsampled.
pub const MAX_TIP_EDGE: u32 = 2500;
/// Largest texture tile edge kept as is.
pub const MAX_TEXTURE_EDGE: u32 = 1024;

/// A brush decoded from one of the supported files, before it becomes a preset.
#[derive(Clone, Debug, Default)]
pub struct ImportedBrush {
    pub name: String,
    /// Tip bitmap (coverage in the card's alpha channel). `None` = a computed round tip.
    pub tip: Option<GrayTile>,
    /// Diameter in pixels.
    pub size: f32,
    /// Dab spacing as a fraction of the diameter (0.25 = 25 %).
    pub spacing: f32,
    pub opacity: f32,
    pub flow: f32,
    /// 0 = soft, 1 = hard. Computed tips only.
    pub hardness: f32,
    pub angle: f32,
    /// 0..1 (1 = circle).
    pub roundness: f32,
    pub pressure_size: bool,
    /// Floor the pen pressure may shrink the size to (0..1).
    pub min_size: f32,
    pub pressure_opacity: bool,
    pub min_opacity: f32,
    pub size_jitter: f32,
    pub angle_jitter: f32,
    pub roundness_jitter: f32,
    /// Scatter amount, 1.0 = 100 %.
    pub scatter: f32,
    pub scatter_count: u32,
    pub scatter_both_axes: bool,
    pub hue_jitter: f32,
    pub saturation_jitter: f32,
    pub brightness_jitter: f32,
    pub fg_bg_jitter: f32,
    /// Wet edge amount (0 = off).
    pub wet_edges: f32,
    /// Colour smudging (0 = off), mapped onto the pigment mix strength.
    pub smudge: f32,
    /// Smoothing amount 0..1.
    pub smoothing: f32,
    pub eraser: bool,
    pub texture: Option<ImportedTexture>,
    pub dual: Option<ImportedDual>,
    /// End taper: (span share 0..1, share of the pressure taken away 0..1).
    pub taper: Option<(f32, f32)>,
}

/// A texture tile a brush stamps through, with the depth/scale the source asked for.
#[derive(Clone, Debug)]
pub struct ImportedTexture {
    pub tile: GrayTile,
    pub depth: f32,
    pub scale: f32,
    pub invert: bool,
    pub brightness: f32,
    pub contrast: f32,
}

/// A second tip stroked along the same path.
#[derive(Clone, Debug)]
pub struct ImportedDual {
    pub tip: Option<GrayTile>,
    pub size: f32,
    pub spacing: f32,
    pub opacity: f32,
    pub flow: f32,
    pub hardness: f32,
    pub roundness: f32,
    pub angle: f32,
    pub mode: MaskMode,
    pub scatter: f32,
    pub both_axes: bool,
    pub count: u32,
}

impl ImportedBrush {
    fn normalised(mut self) -> Self {
        let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
        self.size = finite(self.size, 40.0).clamp(1.0, 5000.0);
        self.spacing = finite(self.spacing, 0.25).clamp(0.01, 10.0);
        self.opacity = finite(self.opacity, 1.0).clamp(0.01, 1.0);
        self.flow = finite(self.flow, 1.0).clamp(0.0, 1.0);
        self.hardness = finite(self.hardness, 1.0).clamp(0.0, 1.0);
        self.roundness = finite(self.roundness, 1.0).clamp(0.01, 1.0);
        self.min_size = finite(self.min_size, 0.0).clamp(0.0, 1.0);
        self.min_opacity = finite(self.min_opacity, 0.0).clamp(0.0, 1.0);
        self.size_jitter = finite(self.size_jitter, 0.0).clamp(0.0, 1.0);
        self.angle_jitter = finite(self.angle_jitter, 0.0).clamp(0.0, 1.0);
        self.roundness_jitter = finite(self.roundness_jitter, 0.0).clamp(0.0, 1.0);
        self.scatter = finite(self.scatter, 0.0).clamp(0.0, 10.0);
        self.scatter_count = self.scatter_count.clamp(1, 16);
        self.hue_jitter = finite(self.hue_jitter, 0.0).clamp(0.0, 1.0);
        self.saturation_jitter = finite(self.saturation_jitter, 0.0).clamp(0.0, 1.0);
        self.brightness_jitter = finite(self.brightness_jitter, 0.0).clamp(0.0, 1.0);
        self.fg_bg_jitter = finite(self.fg_bg_jitter, 0.0).clamp(0.0, 1.0);
        self.wet_edges = finite(self.wet_edges, 0.0).clamp(0.0, 1.0);
        self.smudge = finite(self.smudge, 0.0).clamp(0.0, 1.0);
        self.smoothing = finite(self.smoothing, 0.0).clamp(0.0, 1.0);
        self
    }

    /// The preset this brush becomes in `group` under `name`.
    pub fn preset(self, name: String, group: &str) -> BrushPreset {
        let brush = self.settings(name.as_str());
        BrushPreset { name, brush, builtin: false, group: group.to_string(), folder: Vec::new(), captures: Default::default() }
    }

    /// The brush settings for this brush (also used by tests).
    pub fn settings(mut self, name: &str) -> BrushSettings {
        self = self.normalised();
        let mut b = BrushSettings {
            size: self.size,
            spacing: self.spacing,
            opacity: self.opacity,
            flow: self.flow,
            hardness: self.hardness,
            angle: self.angle,
            roundness: self.roundness,
            tip: self.tip.map_or(TipShape::Round, TipShape::Sampled),
            pressure_size: self.pressure_size,
            pressure_opacity: self.pressure_opacity,
            erase: self.eraser,
            pigment_mix_strength: self.smudge,
            wet_edges: self.wet_edges > 0.0,
            smoothing: photocraft_paint::Smoothing { amount: self.smoothing, ..Default::default() },
            ..Default::default()
        };
        let _ = name;
        if self.pressure_size || self.size_jitter > 0.0 || self.angle_jitter > 0.0 || self.roundness_jitter > 0.0 {
            b.shape_dynamics = ShapeDynamics {
                enabled: true,
                size: Dynamic { jitter: self.size_jitter, control: if self.pressure_size { Control::PenPressure } else { Control::Off }, minimum: self.min_size, ..Dynamic::default() },
                angle: Dynamic { jitter: self.angle_jitter, ..Dynamic::default() },
                roundness: Dynamic { jitter: self.roundness_jitter, ..Dynamic::default() },
                ..Default::default()
            };
        }
        if self.pressure_opacity {
            b.transfer = Transfer {
                enabled: true,
                opacity: Dynamic { control: Control::PenPressure, minimum: self.min_opacity, ..Dynamic::default() },
                ..Default::default()
            };
        }
        if self.scatter > 0.0 || self.scatter_count > 1 {
            b.scattering = Scattering {
                enabled: true,
                scatter: Dynamic { jitter: self.scatter, ..Dynamic::default() },
                both_axes: self.scatter_both_axes,
                count: self.scatter_count.max(1),
                ..Scattering::default()
            };
        }
        if self.hue_jitter > 0.0 || self.saturation_jitter > 0.0 || self.brightness_jitter > 0.0 || self.fg_bg_jitter > 0.0 {
            b.color_dynamics.enabled = true;
            b.color_dynamics.hue_jitter = self.hue_jitter;
            b.color_dynamics.saturation_jitter = self.saturation_jitter;
            b.color_dynamics.brightness_jitter = self.brightness_jitter;
            b.color_dynamics.fg_bg = Dynamic { jitter: self.fg_bg_jitter, ..Dynamic::default() };
        }
        if let Some(t) = self.texture {
            b.texture.enabled = true;
            b.texture.pattern = Pattern::Tile(t.tile);
            b.texture.depth = t.depth.clamp(0.0, 1.0);
            b.texture.scale = t.scale.clamp(0.01, 10.0);
            b.texture.invert = t.invert;
            b.texture.brightness = t.brightness.clamp(-1.0, 1.0);
            b.texture.contrast = t.contrast.clamp(-1.0, 1.0);
            b.texture.mode = MaskMode::Multiply;
            b.texture.each_tip = true;
        }
        if let Some(d) = self.dual {
            b.dual_brush = DualBrush {
                enabled: true,
                tip: d.tip.map_or(TipShape::Round, TipShape::Sampled),
                size: d.size.clamp(1.0, 5000.0),
                spacing: d.spacing.clamp(0.01, 10.0),
                opacity: d.opacity.clamp(0.0, 1.0),
                flow: d.flow.clamp(0.0, 1.0),
                hardness: d.hardness.clamp(0.0, 1.0),
                roundness: d.roundness.clamp(0.01, 1.0),
                angle: d.angle,
                mode: d.mode,
                scatter: d.scatter.clamp(0.0, 10.0),
                both_axes: d.both_axes,
                count: d.count.clamp(1, 16),
                ..Default::default()
            };
        }
        if let Some((end, size)) = self.taper {
            b.taper.enabled = true;
            b.taper.end = end.clamp(0.0, 1.0);
            b.taper.size = size.clamp(0.0, 1.0);
        }
        b
    }
}

/// What one file held, before it is added to the library.
#[derive(Clone, Debug, Default)]
pub struct ParsedFile {
    pub brushes: Vec<ImportedBrush>,
    pub warnings: Vec<String>,
}

/// The presets read from one file plus everything that could not be mapped.
#[derive(Clone, Debug, Default)]
pub struct BrushImport {
    pub presets: Vec<BrushPreset>,
    pub warnings: Vec<String>,
    /// Format name for the status line ("Procreate", "Krita"…).
    pub format: &'static str,
}

/// The format a file is read as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Abr,
    Tpl,
    Gimp,
    MyPaint,
    Krita,
    Procreate,
    Sai2,
    Gobrush,
    MediBang,
    Painter,
    Sut,
    Unknown,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Abr => "Photoshop",
            Format::Tpl => "Photoshop tool presets",
            Format::Gimp => "GIMP",
            Format::MyPaint => "MyPaint",
            Format::Krita => "Krita",
            Format::Procreate => "Procreate",
            Format::Sai2 => "SAI2",
            Format::Gobrush => "天生会画",
            Format::MediBang => "MediBang",
            Format::Painter => "Corel Painter",
            Format::Sut => "CLIP STUDIO PAINT",
            Format::Unknown => "unknown",
        }
    }
}

/// Read any supported brush file. `group` names the library group the presets land in.
pub fn read_any(bytes: &[u8], file_name: &str, group: &str) -> Result<BrushImport, String> {
    read_any_with(bytes, file_name, group, &photocraft_raster::Interrupt::NONE)
}

/// [`read_any`] that checks `ctl` between brushes (a cancelled import fails with "cancelled").
pub fn read_any_with(bytes: &[u8], file_name: &str, group: &str, ctl: &photocraft_raster::Interrupt) -> Result<BrushImport, String> {
    let format = detect(bytes, file_name);
    if format == Format::Unknown {
        return Err(format!("{file_name}: not a brush file PhotoCraft can read (known: .abr, .tpl, .gbr, .gih, .myb, .kpp, .bundle, .brush, .brushset, .sai2, .gobrush, .gobrushes, .mdp, .ini, .sut, Corel Painter library)"));
    }
    let parsed = match format {
        Format::Abr => return abr_import(bytes, group, ctl),
        Format::Tpl => return tpl::read(bytes, group, ctl).map_err(|e| format!("{file_name}: {e}")),
        Format::Gimp => gimp::parse(bytes, file_name).map_err(|e| format!("{file_name}: {e}"))?,
        Format::MyPaint => mypaint::parse(bytes, file_name).map_err(|e| format!("{file_name}: {e}"))?,
        Format::Krita => krita::parse(bytes, file_name).map_err(|e| format!("{file_name}: {e}"))?,
        Format::Procreate => procreate::parse(bytes).map_err(|e| format!("{file_name}: {e}"))?,
        Format::Sai2 => sai2::parse(bytes, file_name).map_err(|e| format!("{file_name}: {e}"))?,
        Format::Gobrush => gobrush::parse(bytes, file_name).map_err(|e| format!("{file_name}: {e}"))?,
        Format::MediBang => medibang::parse(bytes, file_name).map_err(|e| format!("{file_name}: {e}"))?,
        Format::Painter => painter::parse(bytes, file_name).map_err(|e| format!("{file_name}: {e}"))?,
        Format::Sut => csp::parse(bytes, file_name).map_err(|e| format!("{file_name}: {e}"))?,
        Format::Unknown => return Err(format!("{file_name}: unsupported brush format")),
    };
    Ok(assemble(parsed, group, format, ctl))
}

/// The Photoshop path keeps its own reader (it knows folders and sample tables).
fn abr_import(bytes: &[u8], group: &str, ctl: &photocraft_raster::Interrupt) -> Result<BrushImport, String> {
    let imp = crate::abr_map::read_abr_with(bytes, group, ctl)?;
    Ok(BrushImport { presets: imp.presets, warnings: imp.warnings, format: Format::Abr.label() })
}

fn assemble(parsed: ParsedFile, group: &str, format: Format, ctl: &photocraft_raster::Interrupt) -> BrushImport {
    let mut warnings = parsed.warnings;
    let mut presets = Vec::with_capacity(parsed.brushes.len());
    for (i, brush) in parsed.brushes.into_iter().enumerate() {
        if ctl.cancelled() {
            warnings.push("import cancelled".into());
            break;
        }
        ctl.progress((i as f32 / (i as f32 + 1.0)).min(0.99));
        let name = if brush.name.trim().is_empty() { format!("{} Brush {}", format.label(), i + 1) } else { brush.name.trim().to_string() };
        presets.push(brush.preset(name, group));
    }
    BrushImport { presets, warnings, format: format.label() }
}

/// Sniff the format by extension and content. Content wins where it is unambiguous.
pub fn detect(bytes: &[u8], file_name: &str) -> Format {
    if zip::is_zip(bytes) {
        let names = zip::names(bytes);
        if names.is_empty() {
            return Format::Unknown;
        }
        let has = |needle: &str| names.iter().any(|n| n.to_ascii_lowercase().contains(needle));
        return if has("brush.archive") {
            Format::Procreate
        } else if has(".kpp") {
            Format::Krita
        } else if has(".gobrush") {
            Format::Gobrush
        } else if has("brush_bitmap") || has("brush2.ini") {
            Format::MediBang
        } else if has("toolnrm") || has("bristle") || has("blotmap") || has("scatter/") {
            Format::Sai2
        } else if has("painter") || has("brush") || names.iter().any(|n| n.to_ascii_lowercase().ends_with(".xml")) {
            Format::Painter
        } else {
            Format::Unknown
        };
    }
    if bytes.starts_with(b"8BTP") {
        return Format::Tpl;
    }
    if bytes.starts_with(b"SQLite format 3\0") {
        return Format::Sut;
    }
    if gimp::is_gbr(bytes) {
        return Format::Gimp;
    }
    if gimp::is_gih(bytes) {
        return Format::Gimp;
    }
    if krita::is_kpp(bytes) {
        return Format::Krita;
    }
    if mypaint::is_myb(bytes) {
        return Format::MyPaint;
    }
    if gobrush::is_gobrush(bytes) {
        return Format::Gobrush;
    }
    if medibang::is_mdp(bytes) {
        return Format::MediBang;
    }
    if sai2::is_pack(bytes) {
        return Format::Sai2;
    }
    if painter::is_library(bytes) {
        return Format::Painter;
    }
    if mypaint::is_myb_text(&String::from_utf8_lossy(&bytes[..bytes.len().min(4096)])) {
        return Format::MyPaint;
    }
    if medibang::is_ini(bytes) {
        return Format::MediBang;
    }
    if sai2::is_ini(bytes) {
        return Format::Sai2;
    }
    match ext(file_name).as_str() {
        "abr" => Format::Abr,
        "tpl" => Format::Tpl,
        "gbr" | "gih" => Format::Gimp,
        "myb" => Format::MyPaint,
        "kpp" => Format::Krita,
        "bundle" => Format::Krita,
        "brush" | "brushset" => Format::Procreate,
        "sut" => Format::Sut,
        "mdp" | "ini" => Format::MediBang,
        "gobrush" | "gobrushes" => Format::Gobrush,
        _ => Format::Unknown,
    }
}

fn ext(name: &str) -> String {
    std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// A file's stem, used when the file itself carries no brush name.
pub(crate) fn stem_of(name: &str) -> String {
    std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().trim().to_string()).unwrap_or_default()
}

/// A coverage tile from 8-bit samples (0 = no paint, 255 = full paint).
pub(crate) fn tile_from_u8(w: u32, h: u32, px: &[u8]) -> GrayTile {
    let data: Vec<u16> = px.iter().take((w as usize * h as usize).min(px.len())).map(|&v| u16::from(v) * 257).collect();
    fit_tile(GrayTile { width: w, height: h, data: data.into() }, MAX_TIP_EDGE)
}

/// Box-downsample a tile so neither side exceeds `max`.
pub(crate) fn fit_tile(t: GrayTile, max: u32) -> GrayTile {
    if t.width <= max && t.height <= max {
        return t;
    }
    let k = (t.width.max(t.height) as f32 / max as f32).max(1.0);
    let nw = ((t.width as f32 / k).round() as u32).clamp(1, max);
    let nh = ((t.height as f32 / k).round() as u32).clamp(1, max);
    let mut out = vec![0u16; nw as usize * nh as usize];
    for y in 0..nh {
        let y0 = (y as u64 * t.height as u64 / nh as u64) as usize;
        let y1 = (((y as u64 + 1) * t.height as u64 / nh as u64) as usize).max(y0 + 1).min(t.height as usize);
        for x in 0..nw {
            let x0 = (x as u64 * t.width as u64 / nw as u64) as usize;
            let x1 = (((x as u64 + 1) * t.width as u64 / nw as u64) as usize).max(x0 + 1).min(t.width as usize);
            let mut sum = 0u64;
            let mut n = 0u64;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    if let Some(v) = t.data.get(yy * t.width as usize + xx) {
                        sum += u64::from(*v);
                        n += 1;
                    }
                }
            }
            out[y as usize * nw as usize + x as usize] = if n > 0 { (sum / n) as u16 } else { 0 };
        }
    }
    GrayTile { width: nw, height: nh, data: out.into() }
}

/// Decode an image file (PNG, JPEG…) as a coverage tile: luminance × alpha, cropped to the
/// painted bounds the way a sampled tip is.
pub(crate) fn tip_from_image(bytes: &[u8], invert_luma: bool) -> Result<GrayTile, String> {
    tip_from_image_with(bytes, invert_luma, false)
}

/// [`tip_from_image`] with Procreate's rule: coverage is the brightest colour channel
/// (white = opaque, black = transparent), not a weighted luminance.
pub(crate) fn tip_from_image_max(bytes: &[u8], invert_luma: bool) -> Result<GrayTile, String> {
    tip_from_image_with(bytes, invert_luma, true)
}

fn tip_from_image_with(bytes: &[u8], invert_luma: bool, max_channel: bool) -> Result<GrayTile, String> {
    let image = photocraft_codecs::decode(bytes).map_err(|e| e.to_string())?;
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > (1 << 28) {
        return Err(format!("{w}×{h} is not a usable tip size"));
    }
    let n = image.layout().channels();
    let alpha_at = if n == 2 || n == 4 { Some(n - 1) } else { None };
    let mut luma = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let r = image.get(x, y, 0);
            let g = if n >= 3 { image.get(x, y, 1) } else { r };
            let b = if n >= 3 { image.get(x, y, 2) } else { r };
            let a = alpha_at.map_or(1.0, |c| image.get(x, y, c));
            let mut v = if max_channel { r.max(g).max(b) } else { 0.299 * r + 0.587 * g + 0.114 * b }.clamp(0.0, 1.0);
            if invert_luma {
                v = 1.0 - v;
            }
            luma[(y * w + x) as usize] = (v * a * 255.0 + 0.5) as u8;
        }
    }
    let (w, h, luma) = downsampled_luma(w, h, &luma);
    Ok(cropped_tile(w, h, &luma))
}

/// The largest tip a file may contribute: Procreate ships 2048 px shapes, but the brush engine
/// rasterises sampled tips per dab, so an oversized tip turns a stroke into seconds of CPU work.
const MAX_TIP_SIDE: u32 = 1024;

/// Box-average a coverage plane down to at most [`MAX_TIP_SIDE`] on its long side.
fn downsampled_luma(w: u32, h: u32, luma: &[u8]) -> (u32, u32, Vec<u8>) {
    let factor = (w.max(h)).div_ceil(MAX_TIP_SIDE).max(1);
    if factor <= 1 {
        return (w, h, luma.to_vec());
    }
    let (dw, dh) = ((w / factor).max(1), (h / factor).max(1));
    let mut out = vec![0u8; (dw * dh) as usize];
    for y in 0..dh {
        for x in 0..dw {
            let mut sum = 0u32;
            let mut n = 0u32;
            for dy in 0..factor {
                for dx in 0..factor {
                    let (sx, sy) = (x * factor + dx, y * factor + dy);
                    if sx < w && sy < h {
                        sum += u32::from(luma[(sy * w + sx) as usize]);
                        n += 1;
                    }
                }
            }
            out[(y * dw + x) as usize] = (sum / n.max(1)) as u8;
        }
    }
    (dw, dh, out)
}

/// A coverage tile from straight RGBA pixels: the alpha channel is the coverage (stamps that
/// store their shape in alpha, like MediBang's layer images).
pub(crate) fn tip_from_rgba_alpha(w: u32, h: u32, rgba: &[u8]) -> GrayTile {
    if w == 0 || h == 0 || rgba.len() < (w as usize * h as usize * 4) {
        return GrayTile { width: 1, height: 1, data: vec![0u16].into() };
    }
    let mut a = vec![0u8; (w * h) as usize];
    for (i, out) in a.iter_mut().enumerate() {
        *out = rgba[i * 4 + 3];
    }
    let (w, h, a) = downsampled_luma(w, h, &a);
    cropped_tile(w, h, &a)
}

/// Crop a coverage plane to its painted part (a sampled tip should not carry empty margins).
pub(crate) fn cropped_tile(w: u32, h: u32, luma: &[u8]) -> GrayTile {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in 0..h {
        for x in 0..w {
            if luma.get((y * w + x) as usize).copied().unwrap_or(0) > 2 {
                bounds = Some(match bounds {
                    None => (x, y, x + 1, y + 1),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
                });
            }
        }
    }
    let Some((x0, y0, x1, y1)) = bounds else {
        return GrayTile { width: 1, height: 1, data: vec![0u16].into() };
    };
    let (tw, th) = (x1 - x0, y1 - y0);
    let mut out = vec![0u8; (tw * th) as usize];
    for y in 0..th {
        for x in 0..tw {
            out[(y * tw + x) as usize] = luma[((y + y0) * w + (x + x0)) as usize];
        }
    }
    tile_from_u8(tw, th, &out)
}

#[cfg(test)]
mod tests;

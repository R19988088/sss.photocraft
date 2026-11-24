//! Photoshop tool presets (`.tpl`): an `8BTP` container of `8BIM` blocks; the `tptp` block lists
//! tool presets as a name plus a versioned ActionDescriptor. Tools that carry a `Brsh` brush
//! definition become brushes; the descriptors are mapped exactly like an `.abr` preset.

use photocraft_psd::abr::AbrFile;
use photocraft_psd::descriptor::{Descriptor, Id, UnicodeString, Value};

use super::BrushImport;

/// Is this a Photoshop tool presets file?
pub fn is_tpl(bytes: &[u8]) -> bool {
    bytes.starts_with(b"8BTP")
}

struct Block<'a> {
    key: [u8; 4],
    data: &'a [u8],
}

fn blocks(bytes: &[u8]) -> Result<Vec<Block<'_>>, String> {
    if !is_tpl(bytes) || bytes.len() < 12 {
        return Err("the .tpl header is incomplete".into());
    }
    let version = u32::from_be_bytes(bytes[4..8].try_into().map_err(|_| "the .tpl version is damaged")?);
    if !matches!(version, 2 | 3) {
        return Err(format!("tool preset version {version} is not supported"));
    }
    let count = u32::from_be_bytes(bytes[8..12].try_into().map_err(|_| "the .tpl block count is damaged")?);
    if count > 256 {
        return Err("the .tpl block count is unreasonable".into());
    }
    let mut out = Vec::with_capacity(count as usize);
    let mut p = 12;
    for _ in 0..count {
        let sig = bytes.get(p..p + 4).ok_or("a .tpl block is truncated")?;
        if sig != b"8BIM" {
            return Err("a .tpl block has no 8BIM signature".into());
        }
        let key: [u8; 4] = bytes.get(p + 4..p + 8).ok_or("a .tpl block key is truncated")?.try_into().map_err(|_| "a .tpl block key is truncated")?;
        let len = u32::from_be_bytes(bytes.get(p + 8..p + 12).ok_or("a .tpl block length is truncated")?.try_into().map_err(|_| "a .tpl block length is truncated")?) as usize;
        let data = bytes.get(p + 12..p + 12 + len).ok_or("a .tpl block runs past the file")?;
        out.push(Block { key, data });
        p += 12 + len + (4 - len % 4) % 4;
    }
    Ok(out)
}

fn tools(data: &[u8]) -> Result<Vec<(String, Descriptor)>, String> {
    if data.len() < 4 {
        return Err("the .tpl tool list is incomplete".into());
    }
    let count = u32::from_be_bytes(data[0..4].try_into().map_err(|_| "the .tpl tool count is damaged")?);
    if count > 10_000 {
        return Err("the .tpl tool count is unreasonable".into());
    }
    let mut out = Vec::with_capacity(count as usize);
    let mut p = 4usize;
    for _ in 0..count {
        let n = u32::from_be_bytes(data.get(p..p + 4).ok_or("a .tpl tool name is truncated")?.try_into().map_err(|_| "a .tpl tool name is truncated")?) as usize;
        p += 4;
        if n > 1_048_576 || p + 2 * n + 4 > data.len() {
            return Err("a .tpl tool name length is unreasonable".into());
        }
        let units: Vec<u16> = data[p..p + 2 * n].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        let name = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
        p += 2 * n;
        // The tool's descriptor follows, version tag included: `parse_prefix` reads the version
        // itself, so it must not be consumed here (reading it twice landed on the descriptor's
        // empty name and every .tpl failed with "descriptor version 0").
        let rest = data.get(p..).ok_or("a .tpl tool descriptor is truncated")?;
        let (vd, used) = photocraft_psd::descriptor::VersionedDescriptor::parse_prefix(rest).map_err(|e| format!("a tool descriptor could not be read: {e}"))?;
        p += used;
        // Photoshop's UI name can be a localisation key ("$$$/…=Brush 1").
        let name = match name.rsplit_once('=') {
            Some((_, tail)) if name.starts_with("$$$/") => tail.to_string(),
            _ => name,
        };
        out.push((name, vd.descriptor));
    }
    Ok(out)
}

/// The brush keys a tool preset carries when it stores the brush definition flat, with the tool's
/// own class being `Brsh`.
const BRUSH_KEYS: [&str; 6] = ["Dmtr", "Hrdn", "Spcn", "Angl", "Rndn", "Cntc"];

/// The brush definition a tool preset carries, if it is a brush at all. Photoshop writes it as a
/// `Brsh` item (one descriptor, or a list holding one); some exporters put the brush keys straight
/// on the tool descriptor instead, in which case they are collected into a `Brsh` object.
fn brush_of(tool: &Descriptor) -> Option<Descriptor> {
    match tool.get("Brsh") {
        Some(Value::Descriptor(d)) => return Some(d.clone()),
        Some(Value::List(values)) => {
            if let Some(d) = values.iter().find_map(|v| match v {
                Value::Descriptor(d) => Some(d.clone()),
                _ => None,
            }) {
                return Some(d);
            }
        }
        _ => {}
    }
    if tool.class_id.is("Brsh") || BRUSH_KEYS.iter().any(|k| tool.get(k).is_some()) {
        let mut brush = Descriptor::new("Brsh");
        for (k, v) in &tool.items {
            if !k.is("Nm  ") && !k.is("toolOptions") {
                brush.items.push((k.clone(), v.clone()));
            }
        }
        return Some(brush);
    }
    None
}

/// Read a `.tpl` into brush presets.
pub fn read(bytes: &[u8], group: &str, ctl: &photocraft_raster::Interrupt) -> Result<BrushImport, String> {
    let blocks = blocks(bytes)?;
    let mut warnings = Vec::new();
    let mut presets = Vec::new();
    let mut tools = Vec::new();
    for b in blocks.iter().filter(|b| &b.key == b"tptp") {
        tools.extend(tools_of(b.data)?);
    }
    if tools.is_empty() {
        return Err("the .tpl holds no tool presets".into());
    }
    let mut skipped = 0;
    for (name, tool) in tools {
        let Some(brush) = brush_of(&tool) else {
            skipped += 1;
            continue;
        };
        let mut preset = Descriptor::new("brushPreset");
        for (k, v) in &tool.items {
            if k.is("Nm  ") || k.is("toolOptions") || k.is("Brsh") {
                continue;
            }
            preset.items.push((k.clone(), v.clone()));
        }
        preset.items.push((Id::new("Brsh"), Value::Descriptor(brush)));
        let name = if name.trim().is_empty() { "Brush".to_string() } else { name };
        preset.items.push((Id::new("Nm  "), Value::Text(UnicodeString::new(&name))));
        presets.push(preset);
    }
    if skipped > 0 {
        warnings.push(format!("{skipped} tool presets that are not brushes were skipped"));
    }
    if presets.is_empty() {
        return Err("the .tpl holds no brush tool presets".into());
    }
    let file = AbrFile { version: 6, subversion: 1, presets, ..Default::default() };
    let import = crate::abr_map::map_file_with(&file, group, ctl).map_err(|_| "import cancelled".to_string())?;
    warnings.extend(import.warnings);
    if import.presets.len() < file.presets.len() {
        warnings.push("sampled tips stored in the .tpl could not be read; those brushes use a round tip".to_string());
    }
    Ok(BrushImport { presets: import.presets, warnings, format: "Photoshop tool presets" })
}

fn tools_of(data: &[u8]) -> Result<Vec<(String, Descriptor)>, String> {
    tools(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_psd::descriptor::VersionedDescriptor;

    fn brush_object() -> Descriptor {
        Descriptor::new("Brsh")
            .with("Dmtr", Value::UnitFloat { unit: *b"#Pxl", value: 120.0 })
            .with("Spcn", Value::UnitFloat { unit: *b"#Prc", value: 25.0 })
    }

    fn named(mut d: Descriptor, name: &str) -> Descriptor {
        d.items.push((Id::new("Nm  "), Value::Text(UnicodeString::new(name))));
        d
    }

    fn tpl(version: u32, tools: &[Descriptor]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend((tools.len() as u32).to_be_bytes());
        for d in tools {
            let name = match d.get("Nm  ") {
                Some(Value::Text(t)) => t.to_string_lossy(),
                _ => String::new(),
            };
            let units: Vec<u16> = name.encode_utf16().collect();
            body.extend((units.len() as u32).to_be_bytes());
            for u in units {
                body.extend(u.to_be_bytes());
            }
            body.extend(VersionedDescriptor::new(d.clone()).to_bytes());
        }
        let mut out = b"8BTP".to_vec();
        out.extend(version.to_be_bytes());
        out.extend(1u32.to_be_bytes());
        out.extend(b"8BIM");
        out.extend(b"tptp");
        out.extend((body.len() as u32).to_be_bytes());
        out.extend(&body);
        out
    }

    fn read(tools: &[Descriptor]) -> Result<BrushImport, String> {
        super::read(&tpl(2, tools), "Tool Presets", &photocraft_raster::Interrupt::NONE)
    }

    #[test]
    fn a_tool_preset_with_a_brush_item_imports() {
        let tool = named(Descriptor::new("Brsh").with("Brsh", Value::Descriptor(brush_object())), "Kyle Ink");
        let import = read(&[tool]).expect("a brush tool preset imports");
        assert_eq!(import.presets.len(), 1);
        assert_eq!(import.presets[0].name, "Kyle Ink");
        assert_eq!(import.presets[0].brush.size, 120.0);
        assert_eq!(import.presets[0].brush.spacing, 0.25);
    }

    #[test]
    fn a_brush_written_on_the_tool_itself_imports() {
        // Some exporters write the brush keys straight onto a `Brsh` tool, without a `Brsh` item.
        let tool = named(brush_object(), "Flat 40");
        let import = read(&[tool]).expect("a flat brush tool imports");
        assert_eq!(import.presets.len(), 1, "the preset survives the missing `Brsh` item");
        assert_eq!(import.presets[0].name, "Flat 40");
        assert_eq!(import.presets[0].brush.size, 120.0);
    }

    #[test]
    fn a_brush_listed_under_brsh_imports() {
        let tool = named(Descriptor::new("Brsh").with("Brsh", Value::List(vec![Value::Descriptor(brush_object())])), "Listed");
        let import = read(&[tool]).expect("a list-valued brush imports");
        assert_eq!(import.presets.len(), 1);
        assert_eq!(import.presets[0].name, "Listed");
    }

    #[test]
    fn tools_that_are_not_brushes_are_skipped_with_a_warning() {
        let move_tool = named(Descriptor::new("Move"), "Move 1");
        let brush_tool = named(Descriptor::new("Brsh").with("Brsh", Value::Descriptor(brush_object())), "Ink");
        let import = read(&[move_tool, brush_tool]).expect("the brush survives beside a non-brush");
        assert_eq!(import.presets.len(), 1);
        assert_eq!(import.presets[0].name, "Ink");
        assert!(import.warnings.iter().any(|w| w.contains("not brushes were skipped")), "{:?}", import.warnings);
    }

    #[test]
    fn a_tool_presets_file_without_any_brush_says_so() {
        let err = read(&[named(Descriptor::new("Move"), "Move 1")]).expect_err("no brush at all");
        assert!(err.contains("no brush tool presets"), "{err}");
    }

    #[test]
    fn version_three_files_read_the_same() {
        let brush_tool = named(Descriptor::new("Brsh").with("Brsh", Value::Descriptor(brush_object())), "Ink");
        let import = super::read(&tpl(3, &[brush_tool]), "Tool Presets", &photocraft_raster::Interrupt::NONE).expect("version 3 imports");
        assert_eq!(import.presets.len(), 1);
    }
}

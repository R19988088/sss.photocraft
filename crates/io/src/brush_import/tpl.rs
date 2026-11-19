//! Photoshop tool presets (`.tpl`): an `8BTP` container of `8BIM` blocks; the `tptp` block lists
//! tool presets as a name plus a versioned ActionDescriptor. Tools that carry a `Brsh` brush
//! definition become brushes; the descriptors are mapped exactly like an `.abr` preset.

use photocraft_psd::abr::AbrFile;
use photocraft_psd::descriptor::{Descriptor, UnicodeString, Value};

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
        let version = u32::from_be_bytes(data.get(p..p + 4).ok_or("a .tpl descriptor version is missing")?.try_into().map_err(|_| "a .tpl descriptor version is damaged")?);
        if version != 16 {
            return Err(format!("tool descriptor version {version}, expected 16"));
        }
        p += 4;
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

/// Read a `.tpl` into brush presets.
pub fn read(bytes: &[u8], group: &str, ctl: &photocraft_raster::Interrupt) -> Result<BrushImport, String> {
    let blocks = blocks(bytes)?;
    let mut warnings = Vec::new();
    let mut presets = Vec::new();
    let mut tools = Vec::new();
    for b in blocks.iter().filter(|b| &b.key == b"tptp") {
        tools.extend(tools_of(b.data)?);
    }
    let selected: Vec<(String, Descriptor)> = tools.iter().filter(|(_, d)| matches!(d.get("Brsh"), Some(Value::Descriptor(_)))).cloned().collect();
    if selected.len() != tools.len() {
        warnings.push(format!("{} tool presets that are not brushes were skipped", tools.len() - selected.len()));
    }
    if selected.is_empty() {
        return Err("the .tpl holds no brush tool presets".into());
    }
    for (name, tool) in selected {
        let mut preset = Descriptor::new("brushPreset");
        for (k, v) in &tool.items {
            if !k.is("Nm  ") && !k.is("toolOptions") {
                preset.items.push((k.clone(), v.clone()));
            }
        }
        preset.items.push((photocraft_psd::descriptor::Id::new("Nm  "), Value::Text(UnicodeString::new(&name))));
        presets.push(preset);
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

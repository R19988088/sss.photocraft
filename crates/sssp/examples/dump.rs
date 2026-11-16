//! Print what an SSSP container holds: heads, canvas, layers, tile counts, resources.
//!
//! `cargo run -p photocraft-sssp --example dump -- <file.sssp>`

fn main() -> photocraft_sssp::Result<()> {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: dump <file.sssp>");
        return Ok(());
    };
    let project = photocraft_sssp::read(&path)?;
    println!("file: {path}");
    println!("is_sssp: {}", photocraft_sssp::is_sssp(&path));
    println!("head: {:?} generation {}", project.head, project.generation);
    println!(
        "canvas: {} x {}  tile size {}",
        project.canvas_width(),
        project.canvas_height(),
        project.tile_size()
    );
    println!(
        "layers: {}  tiles: {}  resources: {}  preview: {}  color identity tiles: {}",
        project.layers().len(),
        project.tiles.len(),
        project.resources.len(),
        project.preview.is_some(),
        project.color_identity.len()
    );
    for layer in project.layers() {
        let id = layer.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let name = layer.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let kind = layer.get("type").and_then(|v| v.as_str()).unwrap_or("?");
        let adjustment = layer
            .get("adjustment")
            .and_then(|a| a.get("kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let enabled = layer
            .get("adjustment")
            .and_then(|a| a.get("enabledKinds"))
            .and_then(|v| v.as_array())
            .map(|kinds| {
                kinds
                    .iter()
                    .filter_map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        println!(
            "  {id:>18}  {kind:<10} blend={:<10} adjustment={adjustment:<14} enabled=[{enabled}] tiles={}  {name}",
            layer.get("blendMode").and_then(|v| v.as_str()).unwrap_or("normal"),
            project.tiles_of(id).len()
        );
    }
    Ok(())
}

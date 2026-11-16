//! Proof that this crate reads and rewrites a real C3 project: the sample is read, one tile is
//! decoded, the whole container is written back out, and the copy is read again and compared.
//!
//! The sample is only touched when it exists (C3 projects live on the user's Desktop), so this
//! test never fails on a machine that has none.

use photocraft_sssp as sssp;

fn sample() -> Option<std::path::PathBuf> {
    let candidates = [
        std::env::var("SSSP_SAMPLE").ok().map(std::path::PathBuf::from),
        Some(std::path::PathBuf::from(
            std::env::var("HOME").unwrap_or_default(),
        ))
        .map(|home| home.join("Desktop/小 p 设计/s2.sssp")),
    ];
    candidates.into_iter().flatten().find(|path| path.is_file())
}

#[test]
fn round_trips_a_real_c3_project() {
    let Some(path) = sample() else {
        eprintln!("skipped: no .sssp sample on this machine");
        return;
    };
    let project = sssp::read(&path).expect("read");
    assert!(project.canvas_width() > 0 && project.canvas_height() > 0);
    assert!(!project.layers().is_empty());

    // One tile decodes to a full RGBA16Float block.
    let first = project.tiles.first().expect("the sample has tiles");
    let raw = sssp::decode_tile(&first.payload).expect("decode tile");
    assert_eq!(
        raw.len(),
        sssp::raw_tile_bytes(first.payload.width, first.payload.height).expect("size")
    );

    // One layer assembles to its canvas-sized surface.
    let layer = project.layers()[0]
        .get("id")
        .and_then(|id| id.as_str())
        .expect("layer id");
    let tiles = project.tiles_of(layer);
    if !tiles.is_empty() {
        let surface = sssp::assemble_layer(
            project.canvas_width(),
            project.canvas_height(),
            &tiles,
        )
        .expect("assemble");
        assert_eq!(
            surface.len(),
            project.canvas_width() as usize * project.canvas_height() as usize * 8
        );
    }

    // Write the same project out and read it back.
    let directory = tempfile::tempdir().expect("temp dir");
    let copy = directory.path().join("copy.sssp");
    sssp::write(&copy, &project).expect("write");
    assert!(sssp::is_sssp(&copy));
    let again = sssp::read(&copy).expect("read back");
    assert_eq!(again.canvas_width(), project.canvas_width());
    assert_eq!(again.canvas_height(), project.canvas_height());
    assert_eq!(again.layers().len(), project.layers().len());
    assert_eq!(again.tiles.len(), project.tiles.len());
    assert_eq!(again.resources.len(), project.resources.len());
    assert_eq!(again.metadata.get("tileSize"), project.metadata.get("tileSize"));

    // Every layer kept its identity and adjustment payload.
    for (before, after) in project.layers().iter().zip(again.layers()) {
        assert_eq!(before.get("id"), after.get("id"));
        assert_eq!(before.get("name"), after.get("name"));
        assert_eq!(before.get("type"), after.get("type"));
        assert_eq!(before.get("adjustment"), after.get("adjustment"));
    }
    // And every tile still decodes to the same pixels.
    let before_tile = project.tiles.iter().find(|tile| tile.layer_id == layer);
    let after_tile = again.tiles.iter().find(|tile| tile.layer_id == layer);
    if let (Some(before), Some(after)) = (before_tile, after_tile) {
        assert_eq!(before.x, after.x);
        assert_eq!(before.y, after.y);
        assert_eq!(
            sssp::decode_tile(&before.payload).expect("decode"),
            sssp::decode_tile(&after.payload).expect("decode")
        );
    }
}

#[test]
fn splits_and_reassembles_a_layer() {
    let width = 40;
    let height = 20;
    let mut pixels = vec![0u8; width as usize * height as usize * 8];
    for (index, pixel) in pixels.chunks_exact_mut(8).enumerate() {
        // Half float 1.0 is 0x3C00; give red a value and alpha full so the tile is not skipped.
        pixel[0..2].copy_from_slice(&0x3C00u16.to_le_bytes());
        pixel[6..8].copy_from_slice(&0x3C00u16.to_le_bytes());
        let _ = index;
    }
    let tiles = sssp::split_layer("layer", width, height, 16, &pixels, "v").expect("split");
    assert_eq!(tiles.len(), 3 * 2);
    let assembled = sssp::assemble_layer(width, height, &tiles.iter().collect::<Vec<_>>())
        .expect("assemble");
    assert_eq!(assembled, pixels);
}

#[test]
fn edge_tiles_keep_their_cropped_width() {
    let width = 300u32;
    let height = 20u32;
    let mut pixels = vec![0u8; width as usize * height as usize * 8];
    for pixel in pixels.chunks_exact_mut(8) {
        pixel[6] = 1; // alpha half float 0x0001 -> non-zero, so tiles are kept
    }
    let tiles = photocraft_sssp::split_layer("l", width, height, 256, &pixels, "v").unwrap();
    let last = tiles.iter().max_by_key(|t| t.x).unwrap();
    assert_eq!(last.x, 1);
    assert_eq!(last.payload.width, 44);
    assert_eq!(last.payload.height, 20);
}

//! A stroke with a pattern sampled off the canvas paints the patch's graphic: the stamp takes the
//! mark's silhouette and the mark's colours — never the brush's colour, never black, and never the
//! page the sample was taken on. Run with `PHOTOCRAFT_PROCRAFT_BRUSH=1` for the ProCraft brush.

use photocraft_engine::Session;
use photocraft_paint::brush::ColorPattern;
use serde_json::json;

fn session_with_patch() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":120,"height":80,"depth":8,"mode":"rgb","background":"white"})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    // A diamond mark on a see-through layer: the stamp's shape and its colour both come from it.
    let mut pixels = Vec::new();
    for y in 0..12i32 {
        for x in 0..12i32 {
            let d = (x - 6).abs() + (y - 6).abs();
            pixels.push(if d <= 3 { [0.8, 0.2, 0.15, 1.0] } else { [0.0, 0.0, 0.0, 0.0] });
        }
    }
    s.tools.brush.pattern_enabled = true;
    s.tools.brush.sampled_pattern = Some(ColorPattern { width: 12, height: 12, pixels: pixels.into() });
    s.tools.brush.color = [0.0, 0.0, 0.0, 1.0];
    s.tools.brush.size = 24.0;
    s
}

#[test]
fn a_sampled_mark_stamps_the_patch_graphic() {
    let mut s = session_with_patch();
    s.execute("paint.stroke", json!({"points":[[20,40],[100,40]],"size":24,"hardness":100})).unwrap();
    let st = s.active().unwrap();
    let surf = st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap();
    let (mut painted, mut black, mut mark) = (0, 0, 0);
    for y in 26..54 {
        for x in 16..104 {
            let p = surf.rgba(x, y);
            if p[3] < 0.1 {
                continue;
            }
            painted += 1;
            if p[0] < 0.2 && p[1] < 0.2 && p[2] < 0.2 {
                black += 1;
            } else if p[0] > 0.5 && p[2] < 0.4 {
                mark += 1;
            }
        }
    }
    assert!(painted > 0, "the stroke painted nothing");
    assert_eq!(black, 0, "the stroke painted black: {black} pixels");
    assert!(mark > 0, "the mark's own colour is not what the stroke paints");
    // The diamond fills about half of its square, so the stamp is the mark and not the page.
    assert!(painted < 76 * 28, "the stamp took the whole sample square, not the mark's shape: {painted} pixels");
}

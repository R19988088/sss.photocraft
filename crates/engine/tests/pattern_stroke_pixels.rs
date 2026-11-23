//! A stroke with a pattern sampled off the canvas paints the patch's colours — not the brush's own,
//! and not black. Run with `PHOTOCRAFT_PROCRAFT_BRUSH=1` to test the ProCraft brush.

use photocraft_engine::Session;
use photocraft_paint::brush::ColorPattern;
use serde_json::json;

fn session_with_patch() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":120,"height":80,"depth":8,"mode":"rgb","background":"white"})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    // Two bands of colours nothing else in the stroke is. Wide bands on purpose: the pattern sample
    // is an alpha-weighted five-tap read, so a one-texel checkerboard would average into one grey
    // and say nothing about whether the patch is what the stroke paints.
    let mut pixels = Vec::new();
    for _y in 0..8 {
        for x in 0..8 {
            pixels.push(if x < 4 { [0.1, 0.3, 0.9, 1.0] } else { [0.9, 0.8, 0.1, 1.0] });
        }
    }
    s.tools.brush.pattern_enabled = true;
    s.tools.brush.sampled_pattern = Some(ColorPattern { width: 8, height: 8, pixels: pixels.into() });
    s.tools.brush.color = [0.0, 0.0, 0.0, 1.0];
    s.tools.brush.size = 20.0;
    s
}

#[test]
fn a_sampled_pattern_stroke_paints_the_patch() {
    let mut s = session_with_patch();
    s.execute("paint.stroke", json!({"points":[[20,40],[100,40]],"size":20,"hardness":100})).unwrap();
    let st = s.active().unwrap();
    let surf = st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap();
    let mut black = 0;
    let mut blue = 0;
    let mut yellow = 0;
    for y in 30..50 {
        for x in 24..96 {
            let p = surf.rgba(x, y);
            if p[3] < 0.1 {
                continue;
            }
            if p[0] < 0.2 && p[1] < 0.2 && p[2] < 0.2 {
                black += 1;
            } else if p[2] > 0.6 {
                blue += 1;
            } else if p[0] > 0.6 && p[1] > 0.5 {
                yellow += 1;
            }
        }
    }
    assert_eq!(black, 0, "the stroke painted black: {black} pixels");
    assert!(blue > 0 && yellow > 0, "the patch's two colours must both show: blue {blue}, yellow {yellow}");
}

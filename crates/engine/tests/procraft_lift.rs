//! The pen's lift, under ProCraft's brush: the stroke is laid for good as it comes, so the commit
//! takes the live pixels instead of rendering the whole stroke again — and the end the live stroke
//! drew stays, with nothing left outside it.
//!
//! Run with `PHOTOCRAFT_PROCRAFT_BRUSH=1` for the ProCraft brush; without it the case is skipped
//! (the browser's own taper lands at the lift then, which that stroke asks for).

use photocraft_engine::brush_cmds::LiveStroke;
use photocraft_engine::paint::StrokePoint;
use photocraft_engine::Session;
use serde_json::json;

#[test]
fn the_lift_of_a_procraft_stroke_keeps_its_live_pixels() {
    if !photocraft_engine::paint::procraft::enabled() {
        return;
    }
    let mut s = Session::new();
    s.execute("file.new", json!({"width":160,"height":120,"depth":8,"mode":"rgb","background":"white"})).unwrap();
    s.execute("layer.new.layer", json!({})).unwrap();
    s.tools.brush.color = [0.16, 0.32, 0.75, 1.0];
    // The taper a plain brush carries by default: it must not land at the lift under ProCraft.
    let p = json!({"brush": {"size": 20, "taper": {"enabled": true, "size": 0.6, "end": 0.35}}, "target": "pixels"});
    let pts: Vec<[f64; 3]> = (0..24)
        .map(|i| {
            let t = f64::from(i) / 23.0;
            [20.0 + 100.0 * t, 40.0 + (t * std::f64::consts::PI).sin() * 18.0, 1.0 - 0.6 * t]
        })
        .collect();
    let mut live = {
        let mut begin = p.clone();
        begin["points"] = json!([pts[0]]);
        LiveStroke::begin_with(&s, "paint.stroke", &begin).unwrap()
    };
    for q in &pts[1..] {
        let sp = [StrokePoint::new(q[0], q[1], q[2] as f32)];
        live.push(&sp).unwrap();
    }
    let mut commit = p.clone();
    commit["points"] = json!(pts);
    commit["seed"] = json!(live.seed);
    s.offer_live_stroke(live).unwrap();
    let r = s.execute("paint.stroke", commit.clone()).unwrap();
    s.clear_live_stroke();
    assert_eq!(r["fromLive"], true, "the lift rendered the stroke again instead of keeping what it drew");

    // Nothing lies off the stroke: a re-render with a faded, re-pressured end left the live end
    // behind as a broken scrap.
    let st = s.active().unwrap();
    let surf = st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap();
    let mut painted = 0;
    for y in 0..120 {
        for x in 0..160 {
            let c = surf.rgba(x, y);
            if c[3] < 0.05 {
                continue;
            }
            painted += 1;
            let inside = (10..=140).contains(&x) && (18..=72).contains(&y);
            assert!(inside, "a painted pixel at {x},{y} lies off the stroke");
        }
    }
    assert!(painted > 0, "the stroke painted nothing");
}

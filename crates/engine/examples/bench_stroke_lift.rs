//! Pen-up cost of a Brush stroke on a 2520 x 1575 layer, in release mode: the commit rendering the
//! whole stroke again for the end taper against taking the live pixels (`fromLive`), and the live
//! stroke's per-frame cost while drawn. Run:
//! `cargo run --release -p photocraft-engine --example bench_stroke_lift`
//!
//! 2026-11-21 (CPU path, no GPU masker; the app's folded GPU path is cheaper per dab): a 480-point
//! stroke cost 19 ms (40 px round) to 280-300 ms (200 px soft / wet) to lay again, and 1.2-1.5 ms
//! to take the live pixels. Since 2026-11-23 every stroke takes the live pixels: the lift lays no
//! end taper (no pressure post-processing), so the "lay again" column is what the app no longer
//! does at pen-up.
use std::time::Instant;

use photocraft_engine::{Session, brush_cmds::LiveStroke, paint::StrokePoint};
use serde_json::json;

const FRAMES: usize = 120;
const SAMPLES: usize = 4;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn median(v: &mut Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":2520,"height":1575,"depth":8,"mode":"rgb"}))?;
    s.execute("layer.new.layer", json!({}))?;
    // ~1200 px/s at 60 fps: 20 px of drag per frame, in four samples, with a pressure wave.
    let pts: Vec<[f64; 3]> = std::iter::once([100.0, 800.0, 0.6])
        .chain((1..=FRAMES).flat_map(|f| {
            (1..=SAMPLES).map(move |k| {
                let x = 100.0 + (f as f64 - 1.0) * 20.0 + k as f64 * 5.0;
                let y = 800.0 + (x / 260.0).sin() * 220.0;
                let pr = 0.55 + 0.4 * (x / 97.0).sin().abs();
                [x, y, pr]
            })
        }))
        .collect();
    let point_json = serde_json::to_value(&pts)?;
    for (name, brush) in [
        ("round 40 / 25%", json!({"size": 40, "hardness": 0.7, "spacing": 0.25})),
        ("soft 200 / 5%", json!({"size": 200, "hardness": 0.0, "spacing": 0.05, "flow": 0.6})),
        ("soft 200 wet", json!({"size": 200, "hardness": 0.0, "spacing": 0.05, "wetEdges": true, "pigmentMixStrength": 0.6})),
    ] {
        for taper in [true, false] {
            let mut b = brush.as_object().unwrap().clone();
            b.insert("taper".into(), json!({"enabled": taper}));
            let p = json!({"points": point_json, "brush": serde_json::Value::Object(b), "smoothing": 0.3});
            let draw = |s: &Session| -> Result<(LiveStroke, Vec<f64>), Box<dyn std::error::Error>> {
                let mut p0 = p.clone();
                p0["points"] = json!([pts[0]]);
                let mut live = LiveStroke::begin(s, &p0)?;
                let mut frames = Vec::new();
                for batch in pts[1..].chunks(SAMPLES) {
                    let b: Vec<_> = batch.iter().map(|q| StrokePoint::new(q[0], q[1], q[2] as f32)).collect();
                    let t = Instant::now();
                    live.push(&b)?;
                    frames.push(ms(t));
                }
                Ok((live, frames))
            };
            let mut release = Vec::new();
            let mut frames = Vec::new();
            let mut from_live = 0;
            for _ in 0..3 {
                let (live, f) = draw(&s)?;
                frames.extend(f);
                let seed = live.seed;
                s.offer_live_stroke(live)?;
                let mut p1 = p.clone();
                p1["seed"] = json!(seed);
                let t = Instant::now();
                let r = s.execute("paint.stroke", p1)?;
                release.push(ms(t));
                if r.get("fromLive").and_then(|v| v.as_bool()) == Some(true) {
                    from_live += 1;
                }
                s.clear_live_stroke();
                s.execute("edit.undo", json!({}))?;
            }
            frames.sort_by(f64::total_cmp);
            let total: f64 = frames.iter().sum::<f64>() / 3.0;
            println!(
                "{name:16} taper={taper:5}: pen-up={:7.1} ms (fromLive {from_live}/3) | live frames: p50={:5.2} max={:6.2} total={:7.1} ms",
                median(&mut release),
                frames[frames.len() / 2],
                frames.last().copied().unwrap_or(0.0),
                total
            );
        }
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

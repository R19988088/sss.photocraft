//! What a line-art field's backdrop costs: `photocraft_compose::render` over the field rect,
//! over a stack whose layers carry layer styles. Run with
//! `cargo test --release -p photocraft-compose --test field_backdrop_cost -- --nocapture`.
use photocraft_color::{BlendMode, Color, ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Contour, Document, Effect, FxCommon, Layer, LayerContent, Shadow, Size};
use photocraft_geom::Rect;
use photocraft_raster::Surface;

fn filled(w: u32, h: u32, seed: u8) -> Surface {
    let r = Rect::new(0, 0, w as i32, h as i32);
    let mut s = Surface::new(PixelFormat::RGBA8);
    let row: Vec<u8> = (0..w).flat_map(|x| [(x as u8).wrapping_mul(seed), (x / 3) as u8, seed, 200]).collect();
    let bytes: Vec<u8> = (0..h).flat_map(|_| row.iter().copied()).collect();
    s.write_interleaved(r, &bytes);
    s
}

fn shadow() -> Shadow {
    Shadow {
        common: FxCommon::new(BlendMode::Normal, 1.0),
        color: Color::BLACK,
        angle: 120.0,
        use_global_light: false,
        distance: 10.0,
        spread: 0.0,
        size: 8.0,
        contour: Contour::Linear,
        anti_alias: true,
        noise: 0.0,
        knocks_out: false,
    }
}

fn doc(w: u32, h: u32, styles: bool, layers: usize) -> Document {
    let mut d = Document::with_background("bench", Size::new(w, h), ColorMode::Rgb, SampleType::U8, Color::WHITE);
    for i in 0..layers {
        let mut l = Layer::new(format!("l{i}"), LayerContent::Raster(filled(w / 2, h / 8, (i as u8) + 2)));
        if styles {
            l.effects.items = vec![Effect::DropShadow(shadow())];
        }
        d.layers.push(l);
    }
    d
}

fn time<F: FnMut()>(what: &str, rounds: usize, mut f: F) -> f64 {
    // Warm the caches the way a live stroke does after its first frames.
    f();
    let t0 = std::time::Instant::now();
    for _ in 0..rounds {
        f();
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
    println!("  {what}: {ms:.1} ms");
    ms
}

#[test]
#[ignore = "measurement, not a gate: cargo test --release -p photocraft-compose --test field_backdrop_cost -- --ignored --nocapture"]
fn field_backdrop_cost() {
    let (w, h) = (4000u32, 2800u32);
    let field = Rect::new(1000, 1000, 1260, 1260); // a stroke dab's field rect, as `plan_region` gives it
    for (styles, layers) in [(false, 8usize), (true, 8), (true, 20)] {
        let below = doc(w, h, styles, layers);
        println!("{layers} layers, styles {styles}, rect {}x{}:", field.width(), field.height());
        // A stroke moves the damage every frame: each render asks for a fresh rect.
        time("render, moving rect", 6, || {
            let r = Rect::new(field.x0 + 3, field.y0 + 3, field.x1 + 3, field.y1 + 3);
            let _ = photocraft_compose::render(&below, r);
        });
        time("render, same rect", 6, || {
            let _ = photocraft_compose::render(&below, field);
        });
    }
}

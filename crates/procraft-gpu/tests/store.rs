//! A layer's store on a real GPU (skipped where the machine has none): what a stroke keeps with
//! its layer between strokes.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use procraft_engine::document::{Document, LayerId};
use procraft_engine::geom::{Homography, Vec2};
use procraft_engine::stroke::{PickUp, Stamp};
use procraft_gpu::{Canvas, Gpu, Resample, STORE_LIT, StrokeStyle, StrokeTool, TipKind};

fn gpu() -> Option<Gpu> {
    match Gpu::headless() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// Fails the test if the GPU reported validation errors by its end.
struct NoErrors<'a>(&'a Gpu);

impl Drop for NoErrors<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert_eq!(self.0.errors(), 0, "GPU validation errors");
        }
    }
}

fn pixel(px: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

const SIDE: usize = 16;

/// A lightness tip, opaque: its left half a light grey, its right half a dark one.
fn tip() -> Vec<u8> {
    (0..SIDE * SIDE).flat_map(|i| if i % SIDE < SIDE / 2 { [191, 191, 191, 255] } else { [64, 64, 64, 255] }).collect()
}

/// A stroke of blue paint into the layer's store, with the tip above: its stamps lay their own
/// colour where they lie, and their grey over the height `height` opaque.
fn into_store(canvas: &mut Canvas, gpu: &Gpu, layer: LayerId, at: &[(f32, f32)], height: f32) {
    let mut style = StrokeStyle::new(StrokeTool::Smudge, [0.0, 0.0, 1.0], 1.0);
    let tip = tip();
    canvas.use_tip_set(gpu, &mut style, 0x570e, SIDE as u32, &[&tip], TipKind::Lightness);
    style.pull = 0.0;
    style.store = STORE_LIT;
    let pick = PickUp { paint_share: 1.0, here: true, height: Some(height), ..PickUp::default() };
    canvas.begin_stroke(layer, style);
    let stamps: Vec<Stamp> =
        at.iter().map(|&(x, y)| Stamp { pick_up: Some(pick), ..Stamp::round(Vec2::new(x, y), 12.0, 1.0) }).collect();
    canvas.add_stamps(&stamps);
    canvas.end_stroke();
}

/// Red all over the layer, as a plain stroke.
fn red(canvas: &mut Canvas, layer: LayerId) {
    canvas.begin_stroke(layer, StrokeStyle::new(StrokeTool::Paint, [1.0, 0.0, 0.0], 1.0));
    canvas.add_stamps(&[Stamp::round(Vec2::new(32.0, 32.0), 60.0, 1.0)]);
    canvas.end_stroke();
}

#[test]
fn a_stroke_into_a_store_shows_its_paint_lit_by_its_height_and_keeps_both_for_the_next() {
    let Some(gpu) = gpu() else { return };
    let _check = NoErrors(&gpu);
    let mut doc = Document::new(64, 64);
    let mut canvas = Canvas::new(&gpu, &doc).unwrap();
    let layer = doc.current();
    red(&mut canvas, layer);
    into_store(&mut canvas, &gpu, layer, &[(32.0, 32.0)], 1.0);
    canvas.render(&gpu, &mut doc, None);
    let store = canvas.store(layer).cloned().unwrap();
    assert_eq!(store.kind, STORE_LIT);
    assert_eq!(store.planes.len(), 2);
    // the paint kept as laid (blue), the height its grey; the layer shows the paint lighter where
    // the grey is light and darker where it is dark: blue's lightness 0.5 goes to 0.75 and 0.25
    let paint = canvas.read_layer(&gpu, store.planes[0]).unwrap();
    assert_eq!(pixel(&paint, 64, 28, 32), [0, 0, 255, 255]);
    let height = canvas.read_layer(&gpu, store.planes[1]).unwrap();
    assert!(pixel(&height, 64, 28, 32)[0].abs_diff(191) <= 2, "{:?}", pixel(&height, 64, 28, 32));
    let px = canvas.read_layer(&gpu, layer).unwrap();
    let (light, dark) = (pixel(&px, 64, 28, 32), pixel(&px, 64, 36, 32));
    assert!(light[0].abs_diff(128) <= 4 && light[2] == 255 && light[3] == 255, "{light:?}");
    assert!(dark[0] <= 2 && dark[2].abs_diff(128) <= 4 && dark[3] == 255, "{dark:?}");
    // outside the stamps the layer is as it was
    assert_eq!(pixel(&px, 64, 4, 4), [255, 0, 0, 255]);

    // the next stroke lays no height of its own: what it lays is lit by the height the first left
    into_store(&mut canvas, &gpu, layer, &[(32.0, 32.0)], 0.0);
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), Some(&store));
    let again = canvas.read_layer(&gpu, layer).unwrap();
    assert_eq!(pixel(&again, 64, 28, 32), light);

    // undone, the first's store as it was; undone again, none, and the layer red
    canvas.undo();
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), Some(&store));
    canvas.undo();
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), None);
    assert_eq!(pixel(&canvas.read_layer(&gpu, layer).unwrap(), 64, 28, 32), [255, 0, 0, 255]);
    // redone: the store back with what it held
    canvas.redo();
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), Some(&store));
    assert_eq!(pixel(&canvas.read_layer(&gpu, layer).unwrap(), 64, 28, 32), light);
    assert_eq!(pixel(&canvas.read_layer(&gpu, store.planes[0]).unwrap(), 64, 28, 32), [0, 0, 255, 255]);
}

#[test]
fn any_other_change_to_the_layer_dries_its_store_and_undoing_that_brings_it_back() {
    let Some(gpu) = gpu() else { return };
    let _check = NoErrors(&gpu);
    let mut doc = Document::new(64, 64);
    let mut canvas = Canvas::new(&gpu, &doc).unwrap();
    let layer = doc.current();
    let other = doc.add_layer();
    canvas.render(&gpu, &mut doc, None);
    into_store(&mut canvas, &gpu, layer, &[(32.0, 32.0)], 1.0);
    canvas.render(&gpu, &mut doc, None);
    let store = canvas.store(layer).cloned().unwrap();
    let shown = canvas.read_layer(&gpu, layer).unwrap();
    // painting another layer leaves it
    red(&mut canvas, other);
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), Some(&store));
    // a plain stroke on it dries it, the layer as it showed
    canvas.begin_stroke(layer, StrokeStyle::new(StrokeTool::Paint, [0.0, 1.0, 0.0], 1.0));
    canvas.add_stamps(&[Stamp::round(Vec2::new(4.0, 4.0), 2.0, 1.0)]);
    canvas.end_stroke();
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), None);
    assert_eq!(pixel(&canvas.read_layer(&gpu, layer).unwrap(), 64, 28, 32), pixel(&shown, 64, 28, 32));
    canvas.undo();
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), Some(&store));
    // so does clearing it
    canvas.clear_layer(layer);
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), None);
    canvas.undo();
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), Some(&store));
    // the store's planes are kept as long as the history refers to them
    assert!(canvas.read_layer(&gpu, store.planes[1]).is_ok());
}

#[test]
fn a_transform_and_a_flip_move_a_store_with_its_layer() {
    let Some(gpu) = gpu() else { return };
    let _check = NoErrors(&gpu);
    let mut doc = Document::new(64, 64);
    let mut canvas = Canvas::new(&gpu, &doc).unwrap();
    let layer = doc.current();
    into_store(&mut canvas, &gpu, layer, &[(20.0, 32.0)], 1.0);
    canvas.render(&gpu, &mut doc, None);
    let store = canvas.store(layer).cloned().unwrap();
    let shown = canvas.read_layer(&gpu, layer).unwrap();
    let at = |x: f32, y: f32| {
        [Vec2::new(x, y), Vec2::new(x + 64.0, y), Vec2::new(x + 64.0, y + 64.0), Vec2::new(x, y + 64.0)]
    };
    // moved 20 right
    let to_source = Homography::quad_to_quad(at(20.0, 0.0), at(0.0, 0.0)).unwrap().to_f32();
    canvas.begin_transform(layer);
    canvas.set_transform(to_source, Resample::Nearest);
    canvas.end_transform(true);
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), Some(&store));
    let px = canvas.read_layer(&gpu, layer).unwrap();
    assert_eq!(pixel(&px, 64, 36, 32), pixel(&shown, 64, 16, 32));
    assert_eq!(pixel(&canvas.read_layer(&gpu, store.planes[0]).unwrap(), 64, 36, 32), [0, 0, 255, 255]);
    assert_eq!(pixel(&canvas.read_layer(&gpu, store.planes[0]).unwrap(), 64, 16, 32)[3], 0);
    // painting into it there now lights its paint by the height moved with it
    into_store(&mut canvas, &gpu, layer, &[(40.0, 32.0)], 0.0);
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(pixel(&canvas.read_layer(&gpu, layer).unwrap(), 64, 36, 32), pixel(&shown, 64, 16, 32));
    canvas.undo();
    // undone, back where it was
    canvas.undo();
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), Some(&store));
    assert_eq!(pixel(&canvas.read_layer(&gpu, store.planes[0]).unwrap(), 64, 16, 32), [0, 0, 255, 255]);
    // flipped across, its planes with it
    canvas.flip_layers(vec![layer], true);
    canvas.render(&gpu, &mut doc, None);
    assert_eq!(canvas.store(layer), Some(&store));
    assert_eq!(pixel(&canvas.read_layer(&gpu, store.planes[0]).unwrap(), 64, 47, 32), [0, 0, 255, 255]);
    assert_eq!(pixel(&canvas.read_layer(&gpu, layer).unwrap(), 64, 47, 32), pixel(&shown, 64, 16, 32));
}

#[test]
fn a_store_put_back_as_an_artwork_kept_it_goes_on_as_it_was() {
    let Some(gpu) = gpu() else { return };
    let _check = NoErrors(&gpu);
    let mut doc = Document::new(64, 64);
    let mut canvas = Canvas::new(&gpu, &doc).unwrap();
    let layer = doc.current();
    into_store(&mut canvas, &gpu, layer, &[(32.0, 32.0)], 1.0);
    canvas.render(&gpu, &mut doc, None);
    let store = canvas.store(layer).cloned().unwrap();
    let planes: Vec<Vec<u8>> = store.planes.iter().map(|p| canvas.read_layer(&gpu, *p).unwrap()).collect();
    let pixels = canvas.read_layer(&gpu, layer).unwrap();
    // another canvas, as an artwork opens: the layer's pixels, then its store
    let mut doc2 = Document::new(64, 64);
    let mut opened = Canvas::new(&gpu, &doc2).unwrap();
    let layer2 = doc2.current();
    opened.upload_layer(&gpu, layer2, &pixels).unwrap();
    assert!(!opened.put_store(&gpu, layer2, 7, &planes));
    assert!(!opened.put_store(&gpu, layer2, STORE_LIT, &planes[..1]));
    assert!(opened.put_store(&gpu, layer2, STORE_LIT, &planes));
    assert!(!opened.can_undo());
    let kept = opened.store(layer2).cloned().unwrap();
    assert_eq!(opened.read_layer(&gpu, kept.planes[1]).unwrap(), planes[1]);
    // painted into with no height of its own: lit by the height it kept
    into_store(&mut opened, &gpu, layer2, &[(32.0, 32.0)], 0.0);
    opened.render(&gpu, &mut doc2, None);
    assert_eq!(opened.store(layer2), Some(&kept));
    assert_eq!(pixel(&opened.read_layer(&gpu, layer2).unwrap(), 64, 28, 32), pixel(&pixels, 64, 28, 32));
}

#[test]
fn on_an_old_gpu_a_stroke_into_a_store_lays_as_on_a_full_one() {
    let (Ok(full), Ok(old)) = (Gpu::headless(), Gpu::headless_old()) else { return };
    let laid = [&full, &old].map(|gpu| {
        let _check = NoErrors(gpu);
        let mut doc = Document::new(64, 64);
        let mut canvas = Canvas::new(gpu, &doc).unwrap();
        let layer = doc.current();
        red(&mut canvas, layer);
        canvas.render(gpu, &mut doc, None);
        let before = canvas.read_layer(gpu, layer).unwrap();
        let steps = canvas.steps_made();
        into_store(&mut canvas, gpu, layer, &[(24.0, 32.0), (32.0, 32.0), (40.0, 30.0)], 1.0);
        canvas.render(gpu, &mut doc, None);
        let store = canvas.store(layer).cloned().unwrap();
        let planes: Vec<Vec<u8>> = store.planes.iter().map(|p| canvas.read_layer(gpu, *p).unwrap()).collect();
        let pixels = canvas.read_layer(gpu, layer).unwrap();
        assert_ne!(pixels, before);
        assert_eq!(canvas.steps_made(), steps + 1);
        (pixels, planes)
    });
    let [(pixels, planes), (old_pixels, old_planes)] = laid;
    let most = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
    let (layer, paint, height) =
        (most(&pixels, &old_pixels), most(&planes[0], &old_planes[0]), most(&planes[1], &old_planes[1]));
    eprintln!("old GPU against a full one: the layer {layer} at most, the paint {paint}, the height {height}");
    assert!(layer <= 2 && paint <= 2 && height <= 1, "{layer} {paint} {height}");
}

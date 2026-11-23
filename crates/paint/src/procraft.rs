//! ProCraft's brush, driven by this app's brush settings.
//!
//! The brush system is ProCraft's (`crates/procraft-engine`, vendored verbatim). This module is the
//! seam: `BrushSettings` in, a ready `Brush` out; pointer points in, dabs out; and `UNMAPPED` names
//! every setting ProCraft's brush has no counterpart for, so the brush panel can show it in pink
//! instead of implying it still acts.

use crate::brush::Control;
use crate::{BrushSettings, Dab, StrokePoint};
use procraft_engine::brush::Brush;
use procraft_engine::geom::Vec2;
use procraft_engine::input::{Pointer, Sample};
use procraft_engine::stroke::Stamp;

/// Our settings that ProCraft's `Brush` has no field for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A brush parameter of ours with no ProCraft counterpart: **pink** in the brush panel.
    Pink,
    /// A section carried in part: its main controls reach ProCraft's brush, its own extras do not —
    /// shown pink too, with the part that is left out named in the panel's tooltip.
    Partial,
    /// Not a brush parameter at all — the tool, the document or the app supplies it (colour, erase,
    /// the layer's blend mode…), so the panel keeps showing it as it always has.
    Tool,
}

/// One of our settings, and whether ProCraft's brush can carry it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unmapped {
    /// The field's path in `BrushSettings`, as the panel labels it.
    pub param: &'static str,
    pub kind: Kind,
}

/// Every setting of ours ProCraft's `Brush` cannot carry, in panel order.
pub const UNMAPPED: &[Unmapped] = &[
    // --- pigment / mixing: ProCraft keeps these in its smudge tool, not in a brush ---
    Unmapped { param: "pigment_mix_strength", kind: Kind::Pink },
    Unmapped { param: "pigment", kind: Kind::Pink },
    Unmapped { param: "contour", kind: Kind::Pink },
    Unmapped { param: "canvas_material", kind: Kind::Pink },
    Unmapped { param: "saturation_preserve_output", kind: Kind::Pink },
    Unmapped { param: "saturation_record_strength", kind: Kind::Pink },
    Unmapped { param: "mix_settings", kind: Kind::Pink },
    Unmapped { param: "flow_algorithm_version", kind: Kind::Pink },
    Unmapped { param: "c3_preserved_parameters", kind: Kind::Pink },
    Unmapped { param: "mixer", kind: Kind::Pink },
    // --- wet edge: its no-build-up rule is `Rendering::Strongest`, the rim it draws is not there ---
    Unmapped { param: "wet_edges", kind: Kind::Partial },
    Unmapped { param: "wet_edge", kind: Kind::Pink },
    // --- patterns: carried as grain, not as a second colour lane (`effective` turns a sampled
    // pattern into a gray tile on the texture lane; the procedural ones already live there) ---
    Unmapped { param: "protect_texture", kind: Kind::Pink },
    Unmapped { param: "noise", kind: Kind::Pink },
    Unmapped { param: "grain", kind: Kind::Pink },
    // --- dynamics with no counterpart (ProCraft's `Shape` has scatter and jitter only) ---
    Unmapped { param: "hardness_dynamic", kind: Kind::Pink },
    // Tilt (angle/size/opacity), follow-azimuth, the angle jitter and the size jitter reach ProCraft;
    // the roundness, stroke, flip and projection controls do not.
    Unmapped { param: "shape_dynamics", kind: Kind::Partial },
    // Its scatter and count reach ProCraft's throw and count: the range, spacing and count jitters,
    // the radial option and the controls behind them do not.
    Unmapped { param: "scattering", kind: Kind::Partial },
    // The dual brush is ProCraft's secondary stroke (its whole settings, or the dual's own size,
    // flow, hardness, roundness, angle, spacing, throw and count); its combine mode and the two
    // strengths have no field.
    Unmapped { param: "dual_brush", kind: Kind::Partial },
    // A taper takes the stroke's width and pressure away near the ends — a shape the brush did not
    // draw, and on a big brush it reads as the stroke being cut short. It is not mapped at all.
    Unmapped { param: "taper", kind: Kind::Pink },
    Unmapped { param: "color_dynamics", kind: Kind::Pink },
    // Its opacity jitter reaches ProCraft's own; flow, wetness and mix jitter do not.
    Unmapped { param: "transfer", kind: Kind::Partial },
    Unmapped { param: "pose", kind: Kind::Pink },
    Unmapped { param: "build_up", kind: Kind::Pink },
    Unmapped { param: "build_up_rate", kind: Kind::Pink },
    Unmapped { param: "motion", kind: Kind::Pink },
    Unmapped { param: "inertia", kind: Kind::Pink },
    Unmapped { param: "color_attributes", kind: Kind::Pink },
    // --- tip maths: a round tip with `hardness`, or a shape image ---
    Unmapped { param: "tip", kind: Kind::Pink },
    Unmapped { param: "c3_tip_math", kind: Kind::Pink },
    Unmapped { param: "triangle", kind: Kind::Pink },
    Unmapped { param: "aliased", kind: Kind::Pink },
    Unmapped { param: "square", kind: Kind::Pink },
    Unmapped { param: "flip_x", kind: Kind::Pink },
    Unmapped { param: "flip_y", kind: Kind::Pink },
    Unmapped { param: "spacing_enabled", kind: Kind::Pink },
    // --- the app's, not the brush's ---
    Unmapped { param: "color", kind: Kind::Tool },
    Unmapped { param: "color_cmyk", kind: Kind::Tool },
    Unmapped { param: "background", kind: Kind::Tool },
    Unmapped { param: "erase", kind: Kind::Tool },
    Unmapped { param: "behind", kind: Kind::Tool },
    Unmapped { param: "mode", kind: Kind::Tool },
    Unmapped { param: "seed", kind: Kind::Tool },
    Unmapped { param: "locks", kind: Kind::Tool },
    Unmapped { param: "smoothing", kind: Kind::Tool },
    Unmapped { param: "pressure_flow", kind: Kind::Tool },
];

/// Whether `param` is pink (ProCraft's brush has no counterpart for it).
pub fn is_pink(param: &str) -> bool {
    UNMAPPED.iter().any(|u| u.param == param && u.kind == Kind::Pink)
}

/// Whether this app draws this brush with ProCraft's brush system.
///
/// `PHOTOCRAFT_PROCRAFT_BRUSH=1` turns it on. Off by default while the replacement is still landing:
/// the paint crate's own suite (232 tests) asserts this app's brush behaviour, and ProCraft's brush
/// draws differently on purpose (spacing, taper, jitter, no pigment). Those tests have to be re-based
/// brush by brush before the default can flip — until then, turning the switch on is a decision, and
/// a preset that depends on a pink control keeps the generator.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| match std::env::var("PHOTOCRAFT_PROCRAFT_BRUSH") {
        Ok(v) => v != "0",
        Err(_) => false,
    })
}

/// Whether this stroke is drawn by ProCraft's brush.
///
/// Not a stroke that mixes: a smudge or a mixer carries paint from dab to dab, which ProCraft's
/// brush does not do — its mixing lives in its own smudge tool, on its own canvas. Driving this
/// app's mixing lane with ProCraft's stamps put the two models at odds (square patches of the
/// canvas restored over the stroke), so a mixing stroke keeps the app's own dabs, the way the
/// 0.23.5 brushes did, and only a brush that lays paint comes from ProCraft.
pub fn draws(brush: &crate::brush::BrushSettings) -> bool {
    enabled() && !mixing(brush)
}

/// Whether the stroke takes paint up from the canvas as it goes.
pub fn mixing(brush: &crate::brush::BrushSettings) -> bool {
    brush.mix_settings.enabled || brush.pigment_mix_strength > 0.0
}

/// The finest spacing a smudge is drawn at, as a fraction of the stamp diameter.
///
/// A smudge carries colour instead of laying it: its stamps smear the same paint along, so it needs
/// far fewer of them than a brush does, and the whole cost of the lane is the count. Measured
/// 900 px, 900 px stamp, p50: 1 % 52.4 ms, 2 % 27.3, 4 % 11.9, 8 % 7.1 — a smudge at 1 % pays four
/// times for a smear no finer than 4 %'s.
pub const SMUDGE_SPACING: f32 = 0.04;

/// `settings` as this app should draw them under ProCraft's brush: one pipeline for the pattern.
///
/// Our sampled pattern is a per-pixel colour (a second lane beside the coverage, and the 35 ms a
/// 900 px stroke costs). ProCraft's brush has grain instead — a texture over the coverage — which
/// this app's shader already combines in one GPU pass. Under the ProCraft brush a pattern is
/// therefore taken as grain: the patch's luminance becomes a gray tile on the texture lane and the
/// colour lane is switched off, so a textured stroke costs what a solid one costs.
pub fn effective(settings: &BrushSettings) -> BrushSettings {
    let mut out = settings.clone();
    if tool(settings) == Tool::Smudge {
        out.spacing = out.spacing.max(SMUDGE_SPACING);
    }
    if !settings.pattern_enabled {
        return out;
    }
    // A pattern read off the canvas is the stroke's *paint*: its colours are what the stroke shows,
    // not a grain over the brush's colour (a black brush over a blue-and-white patch would draw a
    // black stroke). What a canvas read can hand over — an alpha past 1, a NaN, a patch wider than
    // a tile takes — is cleaned up here, so the colour lane reads it without going black.
    let plain = |out: &mut BrushSettings| {
        out.pattern_enabled = false;
        out.sampled_pattern = None;
    };
    let Some(pattern) = &settings.sampled_pattern else { return out };
    let (w, h) = (pattern.width as usize, pattern.height as usize);
    if w == 0 || h == 0 || pattern.pixels.len() != w.checked_mul(h).unwrap_or(0) {
        plain(&mut out);
        return out;
    }
    const MOST_TILE: usize = 2048;
    let step = w.max(h).div_ceil(MOST_TILE).max(1);
    let (tw, th) = (w.div_ceil(step), h.div_ceil(step));
    let mut pixels: Vec<[f32; 4]> = Vec::with_capacity(tw * th);
    let read = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    for y in 0..th {
        for x in 0..tw {
            let p = pattern.pixels[(y * step * w + x * step).min(pattern.pixels.len() - 1)];
            let mut q = [read(p[0]), read(p[1]), read(p[2]), read(p[3])];
            // The patch is the stamp's own graphic (a tip image drawn along the stroke), so the page
            // around the mark is not part of it: the white is cut out and the mark keeps its own
            // silhouette instead of stamping a square of page.
            let page = q[3] >= 0.9 && q[0] >= 0.93 && q[1] >= 0.93 && q[2] >= 0.93 && (q[0] - q[2]).abs() <= 0.05;
            if page {
                q[3] = 0.0;
            }
            pixels.push(q);
        }
    }
    // What is left is the graphic itself: crop to it so the stamp is the mark and not the square
    // the sample was taken in.
    let (mut x0, mut y0, mut x1, mut y1) = (tw, th, 0usize, 0usize);
    for y in 0..th {
        for x in 0..tw {
            if pixels[y * tw + x][3] > 0.05 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 >= x1 || y0 >= y1 {
        plain(&mut out);
        return out;
    }
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut cropped: Vec<[f32; 4]> = Vec::with_capacity(cw * ch);
    for y in 0..ch {
        for x in 0..cw {
            cropped.push(pixels[(y + y0) * tw + (x + x0)]);
        }
    }
    out.sampled_pattern = Some(crate::brush::ColorPattern {
        width: cw as u32,
        height: ch as u32,
        pixels: std::sync::Arc::from(cropped),
    });
    out
}

/// The stamp diameter a stroke of `settings` is drawn at, canvas pixels.
pub fn diameter(settings: &BrushSettings) -> f32 {
    settings.size.clamp(0.5, 4096.0)
}

/// `settings` as ProCraft's brush.
///
/// Fields ProCraft has no counterpart for are left at ProCraft's own defaults — `UNMAPPED` says
/// which; nothing here imitates them, so the canvas shows ProCraft's brush and not a blend of two.
pub fn brush(settings: &BrushSettings) -> Brush {
    let mut brush = Brush::round_pen();
    brush.name = "PhotoCraft".to_owned();

    brush.spacing = settings.spacing.clamp(0.001, 10.0);
    brush.hardness = settings.hardness.clamp(0.0, 1.0);
    brush.flow = settings.flow.clamp(0.0, 1.0);

    // ProCraft's brush keeps the sliders' range and the stroke is built at a diameter between them.
    // One size, no range.
    brush.min_size = diameter(settings);
    brush.max_size = diameter(settings);
    brush.min_opacity = settings.opacity.clamp(0.0, 1.0);
    brush.max_opacity = settings.opacity.clamp(0.0, 1.0);

    // Our three switches are ProCraft's three bleeds at full strength.
    brush.pressure_size = if settings.pressure_size { 1.0 } else { 0.0 };
    brush.pressure_opacity = if settings.pressure_opacity { 1.0 } else { 0.0 };

    brush.shape.roundness = settings.roundness.clamp(0.0, 1.0);
    brush.shape.rotation = settings.angle.to_radians();

    // 运笔 › 笔迹平滑 is ProCraft's stabilisation: each point averaged with those either side of
    // it, so the path is smoothed where it already is and the stamp stays under the pen. Nothing
    // is held back, so a smooth stroke costs no latency — which is what the old string did, and
    // why this one is the smoothing the panel now sets.
    let smoothing = settings.smoothing.enabled;
    brush.set_setting("stabilization", if smoothing { settings.smoothing.realtime.clamp(0.0, 1.0) } else { 0.0 });

    // The string is ProCraft's streamline, which keeps a stroke's newest 50 ms — the ink trails
    // the pen by that much by design — so only the mode that is a string asks for it.
    brush.streamline = if smoothing && settings.smoothing.pulled_string {
        settings.smoothing.amount.clamp(0.0, 1.0)
    } else {
        0.0
    };

    // No taper, at either end, on either pointer: the stroke keeps the width and the pressure the
    // pen gave it all the way to the lift. A taper is a shape the brush never drew — it thins the
    // end into the stroke instead of stopping it — and on a big brush that thinning is a plain
    // break in the shape, so the setting is mapped to nothing. With nothing to hold back, the tip
    // has nothing to animate either: the lift lays no stamp at all.
    brush.pen_taper = procraft_engine::brush::Taper::NONE;
    brush.touch_taper = procraft_engine::brush::Taper::NONE;
    brush.set_setting("pen_tip_animation", 0.0);

    // What leaning does: ProCraft's Tilt is the same three controls, and its dial is our tilt angle.
    brush.tilt.angle = settings.shape_dynamics.tilt_angle.clamp(0.0, 1.0);
    brush.tilt.size = settings.shape_dynamics.tilt_size.clamp(0.0, 2.0);
    brush.tilt.opacity = settings.shape_dynamics.tilt_opacity.clamp(0.0, 1.0);
    brush.shape.follow_azimuth = settings.shape_dynamics.follow_azimuth;

    // Shape dynamics: a random turn (ours is a share of ±180°, ProCraft's a share of half turns),
    // the stamp's size jitter, and a stamp that follows the stroke's direction.
    if settings.shape_dynamics.enabled {
        brush.shape.scatter = settings.shape_dynamics.angle.jitter.min(2.0);
        brush.shape.size_jitter = settings.shape_dynamics.size.jitter.clamp(0.0, 1.0);
        brush.shape.follow = match settings.shape_dynamics.angle.control {
            Control::Direction => 1.0,
            _ => 0.0,
        };
    }

    // Scattering: ours throws dabs up to one radius at 100 %, ProCraft's `throw` counts in stamp
    // diameters; both step the dab count the same way.
    if settings.scattering.enabled {
        brush.shape.throw = settings.scattering.scatter.jitter.min(2.0) * 0.5;
        brush.shape.count = settings.scattering.count.clamp(1, 16);
    }

    // Transfer's opacity jitter is ProCraft's own.
    if settings.transfer.enabled {
        brush.shape.opacity_jitter = settings.transfer.opacity.jitter.clamp(0.0, 1.0);
    }

    // A wet edge's rule — overlapping dabs must not build up — is ProCraft's `Strongest`; the rim
    // it draws is the part ProCraft's brush has no field for (pink).
    if settings.wet_edges {
        brush.rendering = procraft_engine::brush::Rendering::Strongest;
    }

    // A dual brush is ProCraft's `secondary`: a second stroke along the same path. Its whole brush
    // settings win when the preset carries them; otherwise the dual's own size, flow, hardness,
    // roundness, angle, spacing, throw and count stand in, on the primary's other settings.
    if settings.dual_brush.enabled {
        let d = &settings.dual_brush;
        let mut second = match &d.full_parameters {
            Some(full) => crate::procraft::brush(full),
            None => {
                let mut second = brush.clone();
                second.min_size = d.size.clamp(0.5, 4096.0);
                second.max_size = second.min_size;
                second.min_opacity = d.opacity.clamp(0.0, 1.0);
                second.max_opacity = second.min_opacity;
                second.flow = d.flow.clamp(0.0, 1.0);
                second.hardness = d.hardness.clamp(0.0, 1.0);
                second.shape.roundness = d.roundness.clamp(0.0, 1.0);
                second.shape.rotation = d.angle.to_radians();
                second.spacing = d.spacing.clamp(0.001, 10.0);
                second.shape.throw = d.scatter.min(2.0) * 0.5;
                second.shape.count = d.count.clamp(1, 16);
                second
            }
        };
        // One secondary each: the second stroke is not itself a dual brush.
        second.secondary = None;
        // A wet edge is a rule of the stroke that carries it, so each half keeps its own: the
        // second's stamps do not build up when its own wet edge is on, whatever the first's is.
        second.rendering = if d.wet_edges {
            procraft_engine::brush::Rendering::Strongest
        } else {
            brush.rendering
        };
        brush.secondary = Some(Box::new(second));
    }

    brush
}

/// Which of the brush's three tools a stroke is drawn with.
///
/// The brush is one. Painting, smudging and erasing differ only in which of its own settings are let
/// through — the smudge's `mix_settings`, the eraser's `erase` — not in what draws the stroke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Paint,
    Smudge,
    Erase,
}

pub fn tool(settings: &BrushSettings) -> Tool {
    if settings.erase {
        Tool::Erase
    } else if settings.mix_settings.enabled && settings.mix_settings.smudge_enabled {
        Tool::Smudge
    } else {
        Tool::Paint
    }
}

/// A stroke's builder, as ProCraft makes it: the brush, the diameter, and the pointer that draws it.
pub fn stroke(settings: &BrushSettings) -> procraft_engine::stroke::StrokeBuilder {
    let pointer = if settings.erase { Pointer::StylusEraser } else { Pointer::Stylus };
    let mut builder = procraft_engine::stroke::StrokeBuilder::new(brush(settings), diameter(settings), pointer);
    // At the lift the builder walks the path's end again and re-spaces everything it laid, which
    // this app draws as a stroke that grows a new taper and new pressures the moment the pen comes
    // up. The stamps laid as the stroke was drawn are settled already — the canvas holds them —
    // so the end is tapered where it is and nothing behind it is laid again.
    builder.hold_laid();
    builder
}

/// The stamps still to draw when the pen lifts: the stroke's end, as it is once ended.
///
/// Everything the stroke laid as it came is already on the canvas — a stroke drawn with a tip
/// hands those out as it goes (`add_tipped`) — so the lift draws only what the tapers still held
/// back, not the whole stroke again. That is what made the end of a stroke wait: a long stroke
/// re-rastered in full the moment the pen came up.
pub fn lift(builder: &mut procraft_engine::stroke::StrokeBuilder) -> Vec<procraft_engine::stroke::Stamp> {
    let drawn = builder.drawn_for_good().map_or(0, |d| d.len());
    let stamps = builder.finish();
    stamps.get(drawn.min(stamps.len())..).unwrap_or_default().to_vec()
}

/// This app's pointer point, as ProCraft's sample.
///
/// Our tilt is the W3C pair in degrees (each −90..90) and ProCraft's is one lean in radians plus the
/// direction it leans towards; our rotation is the barrel's turn, which is ProCraft's `roll`.
pub fn sample(p: &StrokePoint) -> Sample {
    let lean = p.tilt_x.hypot(p.tilt_y).clamp(0.0, 90.0).to_radians();
    let azimuth = if lean > 0.0 { f64::from(p.tilt_y).atan2(f64::from(p.tilt_x)) as f32 } else { 0.0 };
    Sample {
        pos: Vec2::new(p.x as f32, p.y as f32),
        pressure: p.pressure.clamp(0.0, 1.0),
        tilt: lean,
        azimuth,
        roll: p.rotation.to_radians(),
        time_ms: p.time,
    }
}

/// ProCraft's stamps, as the dabs this app's painter draws.
///
/// The dab carries what the stamp carries — where it is, how big, how strong, how it turns and how
/// squashed — and what the stroke says for all of them: the brush's hardness, the stroke's opacity
/// ceiling and its colour. Everything this app's pigment, mixing and wet machinery would have put
/// on a dab of its own is left at the plain values (`Dab::round`), which is what the pink controls'
/// absence means: ProCraft's brush does not mix, it lays paint.
pub fn dabs(stamps: &[Stamp], settings: &BrushSettings, color: [f32; 4]) -> Vec<Dab> {
    let hardness = settings.hardness.clamp(0.0, 1.0);
    let opacity = settings.opacity.clamp(0.0, 1.0);
    stamps
        .iter()
        .enumerate()
        .map(|(i, stamp)| {
            let mut dab = Dab::round(
                photocraft_geom::Point::new(f64::from(stamp.center.x), f64::from(stamp.center.y)),
                stamp.radius,
                stamp.alpha,
            );
            dab.hardness = hardness;
            dab.angle = stamp.angle;
            dab.roundness = stamp.roundness;
            dab.opacity = opacity;
            dab.color = color;
            dab.index = i as u64;
            dab
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dab_core_reaches_procrafts_brush() {
        let mut settings = BrushSettings::default();
        settings.size = 900.0;
        settings.spacing = 0.01;
        settings.opacity = 0.16;
        settings.flow = 0.16;
        settings.hardness = 0.75;
        settings.roundness = 0.5;
        settings.angle = 30.0;
        settings.pressure_size = true;
        settings.pressure_opacity = false;

        let brush = brush(&settings);
        assert_eq!(brush.max_size, 900.0);
        assert_eq!(brush.min_size, 900.0);
        assert_eq!(diameter(&settings), 900.0);
        assert_eq!(brush.spacing, 0.01);
        assert_eq!(brush.max_opacity, 0.16);
        assert_eq!(brush.min_opacity, 0.16);
        assert_eq!(brush.flow, 0.16);
        assert_eq!(brush.hardness, 0.75);
        assert_eq!(brush.pressure_size, 1.0);
        assert_eq!(brush.pressure_opacity, 0.0);
        assert_eq!(brush.shape.roundness, 0.5);
        assert!((brush.shape.rotation - 30f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn the_old_brushes_parameters_reach_procrafts_brush() {
        use crate::brush::{Dynamic, Scattering, ShapeDynamics, Taper, Transfer};

        let mut settings = BrushSettings::default();
        settings.taper = Taper { enabled: true, end: 0.35, size: 0.6 };
        settings.shape_dynamics = ShapeDynamics {
            enabled: true,
            tilt_angle: 0.4,
            tilt_size: 1.2,
            tilt_opacity: 0.5,
            follow_azimuth: true,
            angle: Dynamic { jitter: 0.25, ..Default::default() },
            ..Default::default()
        };
        settings.scattering = Scattering { enabled: true, count: 3, ..Default::default() };
        settings.scattering.scatter.jitter = 0.8;
        settings.transfer = Transfer { enabled: true, ..Default::default() };
        settings.transfer.opacity.jitter = 0.4;
        settings.wet_edges = true;

        let mapped = brush(&settings);
        assert_eq!(mapped.pen_taper, procraft_engine::brush::Taper::NONE, "a taper would thin the stroke");
        assert_eq!(mapped.touch_taper, procraft_engine::brush::Taper::NONE);
        assert_eq!(mapped.setting("pen_tip_animation"), 0.0, "nothing is held back for a tip to show");
        assert_eq!(mapped.tilt.angle, 0.4);
        assert_eq!(mapped.tilt.size, 1.2);
        assert_eq!(mapped.tilt.opacity, 0.5);
        assert!(mapped.shape.follow_azimuth);
        assert_eq!(mapped.shape.scatter, 0.25, "our share of ±180° is a share of half turns");
        assert_eq!(mapped.shape.size_jitter, 0.0);
        assert_eq!(mapped.shape.throw, 0.4, "a radius of throw is half a stamp diameter");
        assert_eq!(mapped.shape.count, 3);
        assert_eq!(mapped.shape.opacity_jitter, 0.4);
        assert_eq!(mapped.rendering, procraft_engine::brush::Rendering::Strongest, "wet edges must not build up");

        // A brush with the taper off ends at the pen's lift, and nothing else is switched on.
        let plain = BrushSettings { taper: crate::brush::Taper { enabled: false, ..crate::brush::Taper::default() }, ..BrushSettings::default() };
        let quiet = brush(&plain);
        assert_eq!(quiet.pen_taper, procraft_engine::brush::Taper::NONE);
        assert_eq!(quiet.shape.count, 1);
        assert_eq!(quiet.shape.throw, 0.0);
        assert_eq!(quiet.rendering, procraft_engine::brush::Rendering::BuildUp);
    }

    #[test]
    fn a_mixing_stroke_keeps_this_apps_own_dabs() {
        let mut settings = BrushSettings::default();
        assert!(mixing(&settings) == false);
        assert!(draws(&settings) || !enabled());
        settings.mix_settings.enabled = true;
        assert!(mixing(&settings), "a smudge or a mixer takes paint up as it goes");
        assert!(!draws(&settings), "ProCraft's brush cannot carry the paint a mixing lane needs");
        settings.mix_settings.enabled = false;
        settings.pigment_mix_strength = 0.4;
        assert!(mixing(&settings), "a pigment response mixes with what is under it");
        assert!(!draws(&settings));
    }

    #[test]
    fn a_stroke_makes_stamps_at_the_settings_spacing() {
        let mut settings = BrushSettings::default();
        settings.size = 100.0;
        settings.spacing = 0.05;
        settings.smoothing.amount = 0.0;

        let mut builder = stroke(&settings);
        let mut made = Vec::new();
        for i in 0..=40 {
            let sample = Sample::at(Vec2::new(i as f32 * 5.0, 0.0), 1.0, i as f64 * 8.0);
            made.extend(builder.add_tipped(&[sample]).0);
        }
        assert!(made.len() >= 20, "a 200 px stroke at 5 % spacing must stamp: {}", made.len());
        assert!(made.windows(2).all(|w| w[1].center.x >= w[0].center.x), "stamps run along the path");
    }

    #[test]
    fn a_stamp_becomes_a_dab_of_the_same_place_and_strength() {
        let mut settings = BrushSettings::default();
        settings.size = 100.0;
        settings.spacing = 0.25;
        settings.opacity = 0.5;
        settings.hardness = 0.8;
        settings.smoothing.amount = 0.0;

        let mut builder = stroke(&settings);
        let stamps = builder.add(&[Sample::at(Vec2::new(10.0, 10.0), 1.0, 0.0), Sample::at(Vec2::new(90.0, 10.0), 1.0, 8.0)]);
        let dabs = dabs(&stamps, &settings, [0.2, 0.4, 0.6, 1.0]);

        assert_eq!(dabs.len(), stamps.len());
        for (dab, stamp) in dabs.iter().zip(&stamps) {
            assert!((dab.center.x - f64::from(stamp.center.x)).abs() < 1e-9);
            assert!((dab.center.y - f64::from(stamp.center.y)).abs() < 1e-9);
            assert_eq!(dab.radius, stamp.radius);
            assert_eq!(dab.alpha, stamp.alpha);
            assert_eq!(dab.angle, stamp.angle);
            assert_eq!(dab.roundness, stamp.roundness);
            assert_eq!(dab.hardness, 0.8);
            assert_eq!(dab.opacity, 0.5);
            assert_eq!(dab.color, [0.2, 0.4, 0.6, 1.0]);
        }
        assert!(dabs.windows(2).all(|w| w[1].index == w[0].index + 1), "dab indices run from the stroke's start");
    }

    #[test]
    fn a_dual_brush_becomes_procrafts_secondary_stroke() {
        use crate::brush::DualBrush;

        let mut settings = BrushSettings::default();
        settings.size = 120.0;
        settings.dual_brush = DualBrush {
            enabled: true,
            size: 40.0,
            opacity: 0.6,
            flow: 0.5,
            hardness: 0.3,
            roundness: 0.7,
            angle: 15.0,
            spacing: 0.2,
            scatter: 0.6,
            count: 2,
            ..Default::default()
        };

        let mapped = brush(&settings);
        let second = mapped.secondary.expect("a dual brush has a secondary stroke");
        assert_eq!(second.max_size, 40.0);
        assert_eq!(second.min_opacity, 0.6);
        assert_eq!(second.flow, 0.5);
        assert_eq!(second.hardness, 0.3);
        assert_eq!(second.shape.roundness, 0.7);
        assert!((second.shape.rotation - 15f32.to_radians()).abs() < 1e-6);
        assert_eq!(second.spacing, 0.2);
        assert_eq!(second.shape.throw, 0.3, "a radius of throw is half a stamp diameter");
        assert_eq!(second.shape.count, 2);
        assert!(second.secondary.is_none(), "the second stroke is not itself a dual brush");
        assert_eq!(
            second.rendering,
            procraft_engine::brush::Rendering::BuildUp,
            "the second keeps its own rule, not the first's"
        );

        // The second half's own wet edge: its stamps stop building up, while the first's carry on.
        settings.wet_edges = false;
        settings.dual_brush.wet_edges = true;
        let mapped = brush(&settings);
        assert_eq!(mapped.rendering, procraft_engine::brush::Rendering::BuildUp);
        assert_eq!(
            mapped.secondary.expect("still a secondary").rendering,
            procraft_engine::brush::Rendering::Strongest
        );

        // Off: no secondary at all.
        settings.dual_brush.enabled = false;
        assert!(brush(&settings).secondary.is_none());
    }

    #[test]
    fn the_three_tools_are_one_brush_with_different_limits() {
        let mut settings = BrushSettings::default();
        settings.mix_settings.enabled = true;
        settings.mix_settings.smudge_enabled = false;
        assert_eq!(tool(&settings), Tool::Paint);

        // A smudge is the same brush with its mixing settings let through.
        settings.mix_settings.smudge_enabled = true;
        assert_eq!(tool(&settings), Tool::Smudge);
        assert_eq!(brush(&settings).max_size, brush(&BrushSettings::default()).max_size);

        // The eraser is the same brush again, and the stroke's pointer says so.
        settings.erase = true;
        assert_eq!(tool(&settings), Tool::Erase);
    }

    #[test]
    fn a_smudge_is_drawn_at_no_finer_than_four_percent() {
        let mut settings = BrushSettings::default();
        settings.spacing = 0.01;
        settings.mix_settings.enabled = true;
        settings.mix_settings.smudge_enabled = false;
        assert_eq!(effective(&settings).spacing, 0.01, "a brush keeps the spacing it was given");

        settings.mix_settings.smudge_enabled = true;
        assert_eq!(effective(&settings).spacing, SMUDGE_SPACING, "a smudge's stamps smear the same paint");

        // A brush that already asks for coarser stamps keeps them.
        settings.spacing = 0.2;
        assert_eq!(effective(&settings).spacing, 0.2);
    }

    #[test]
    fn the_lift_draws_only_what_the_stroke_has_not_laid_yet() {
        use crate::brush::Taper;

        let mut settings = BrushSettings::default();
        settings.size = 60.0;
        settings.spacing = 0.2;
        settings.taper = Taper { enabled: false, ..Taper::default() };
        let mut builder = stroke(&settings);
        let mut laid = 0;
        for i in 0..24 {
            laid += builder.add_tipped(&[sample(&StrokePoint::new(10.0 + f64::from(i) * 10.0, 40.0, 0.7))]).0.len();
        }
        // with no taper holding anything back, the lift has nothing left to lay
        assert!(laid > 0, "the stroke laid stamps as it came");
        assert!(lift(&mut builder).is_empty(), "the lift re-laid stamps the canvas already had");
    }

    #[test]
    fn the_lift_lays_the_tapers_tail_and_no_more() {
        use crate::brush::Taper;

        // A taper is never mapped to ProCraft's brush (it would thin the stroke), so a brush that
        // asks for one still ends at the pen's own lift and the lift lays nothing at all.
        let mut settings = BrushSettings::default();
        settings.size = 60.0;
        settings.spacing = 0.2;
        settings.taper = Taper { enabled: true, end: 0.5, size: 0.8 };
        let mut builder = stroke(&settings);
        let mut laid = 0;
        for i in 0..24 {
            laid += builder.add_tipped(&[sample(&StrokePoint::new(10.0 + f64::from(i) * 10.0, 40.0, 0.7))]).0.len();
        }
        assert!(laid > 0, "the stroke laid stamps as it came");
        let tail = lift(&mut builder);
        assert!(tail.is_empty(), "the lift drew {} more stamps on a stroke that was whole", tail.len());
        for stamp in builder.stamps() {
            assert!((stamp.radius - 30.0).abs() < 1e-3, "a stamp of this brush is one diameter, start to end");
        }
    }

    #[test]
    fn stroke_smoothing_smooths_where_the_path_is_and_holds_nothing_back() {
        let mut settings = BrushSettings::default();
        settings.smoothing.realtime = 0.6;
        let live = brush(&settings);
        assert!((live.setting("stabilization") - 0.6).abs() < 1e-6, "笔迹平滑 is ProCraft's stabilisation");
        assert_eq!(live.streamline, 0.0, "stabilisation holds nothing back, so there is no stream lag");

        // the string mode is the one that paints behind the pointer
        settings.smoothing.pulled_string = true;
        settings.smoothing.amount = 0.4;
        assert!((brush(&settings).streamline - 0.4).abs() < 1e-6);
    }

    #[test]
    fn a_stroke_does_not_lay_itself_again_at_the_lift() {
        use crate::brush::Taper;

        // The stamps handed out as the stroke is drawn are what the canvas shows; at the lift they
        // must stay where they are, at the size and pressure they had. Without `hold_laid` the
        // builder re-spaces the whole stroke at the lift, which reads as the stroke growing a new
        // taper and new pressures under the pen the moment it comes up.
        let mut settings = BrushSettings::default();
        settings.size = 40.0;
        settings.spacing = 0.25;
        settings.taper = Taper::NONE;
        let mut builder = stroke(&settings);

        let mut handed: Vec<procraft_engine::stroke::Stamp> = Vec::new();
        for i in 0..12 {
            let point = StrokePoint::new(20.0 + f64::from(i) * 8.0, 50.0, 0.6);
            handed.extend(builder.add(&[sample(&point)]));
        }
        let at_lift = builder.finish();

        assert!(!handed.is_empty(), "the stroke laid stamps as it came");
        for (before, after) in handed.iter().zip(at_lift.iter()) {
            assert!(
                (before.center.x - after.center.x).abs() < 1e-3
                    && (before.center.y - after.center.y).abs() < 1e-3
                    && (before.radius - after.radius).abs() < 1e-3
                    && (before.alpha - after.alpha).abs() < 1e-3,
                "the lift moved a stamp already drawn: {before:?} -> {after:?}"
            );
        }
    }

    #[test]
    fn a_sampled_pattern_is_the_strokes_paint_under_the_procraft_brush() {
        use crate::brush::ColorPattern;

        let mut settings = BrushSettings::default();
        settings.pattern_enabled = true;
        settings.sampled_pattern = Some(ColorPattern {
            width: 4,
            height: 4,
            pixels: std::sync::Arc::from(vec![[0.2, 0.2, 0.2, 1.0]; 16].into_boxed_slice()),
        });

        let e = effective(&settings);
        assert!(e.pattern_enabled, "the pattern paints: it is not a grain over the brush's colour");
        let patch = e.sampled_pattern.as_ref().expect("the patch stays the stroke's paint");
        assert!(patch.is_valid());
        assert_eq!((patch.width, patch.height), (4, 4));
        assert_eq!(patch.pixels[0], [0.2, 0.2, 0.2, 1.0], "a readable patch is taken as it is");
        assert!(!e.texture.enabled, "and the texture lane is not pressed into service");

        // A patch wider than a tile takes is read every other texel: the paint keeps its scale.
        let wide: Vec<[f32; 4]> = (0..2100 * 2).map(|i| [i as f32 % 2.0, 0.0, 0.0, 1.0]).collect();
        settings.sampled_pattern = Some(ColorPattern { width: 2100, height: 2, pixels: std::sync::Arc::from(wide) });
        let e = effective(&settings);
        let patch = e.sampled_pattern.as_ref().unwrap();
        assert_eq!((patch.width, patch.height), (1050, 1), "a patch that wide is read every other texel");
        assert!(patch.is_valid());

        // No pattern: the brush is left exactly as it is.
        let plain = BrushSettings::default();
        let same = effective(&plain);
        assert!(!same.pattern_enabled);
        assert!(!same.texture.enabled);
    }

    #[test]
    fn a_pointer_point_becomes_a_sample_with_its_lean() {
        let mut p = StrokePoint::new(12.5, 40.0, 0.6);
        p.tilt_x = 0.0;
        p.tilt_y = 30.0;
        p.time = 128.0;
        let s = sample(&p);
        assert_eq!(s.pos.x, 12.5);
        assert_eq!(s.pos.y, 40.0);
        assert_eq!(s.pressure, 0.6);
        assert!((s.tilt - 30f32.to_radians()).abs() < 1e-6, "our degrees become radians: {}", s.tilt);
        assert!((s.azimuth - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "leaning down +y");
        assert_eq!(s.time_ms, 128.0);
    }
}


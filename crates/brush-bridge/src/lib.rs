//! Our brush settings, read as ProCraft's `Brush`.
//!
//! The brush system is ProCraft's (vendored verbatim in `crates/procraft-engine` and
//! `crates/procraft-gpu`); this crate is the one seam between it and this app: our presets
//! (`photocraft_paint::brush::BrushSettings`) go in, a ready `Brush` comes out, and
//! `unmapped()` names every setting ProCraft's brush has no counterpart for so the brush panel
//! can show it in pink instead of pretending it does something.
//!
//! What is *not* here: colour, alphas of the layer, the document. Those stay this app's; ProCraft's
//! `StrokeBuilder` only turns pointer samples into stamps, and the stamps are drawn by the GPU.

use photocraft_paint::brush::BrushSettings;
use photocraft_paint::Dab;
use procraft_engine::brush::Brush;
use procraft_engine::stroke::Stamp;

/// Our settings that ProCraft's `Brush` has no field for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A brush parameter of ours with no ProCraft counterpart: **pink** in the brush panel.
    Pink,
    /// Not a brush parameter at all — the tool, the document or the app supplies it (colour, erase,
    /// the layer's blend mode…). The brush panel keeps showing it as it always has.
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
///
/// The pink ones are the honest cost of the replacement: those controls may still be shown and
/// edited (presets keep them), but ProCraft's brush will not act on them until they are mapped.
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
    // --- wet edge: no field at all ---
    Unmapped { param: "wet_edges", kind: Kind::Pink },
    Unmapped { param: "wet_edge", kind: Kind::Pink },
    // --- patterns: ours are procedural, ProCraft's grain wants an image from its registry ---
    Unmapped { param: "pattern_enabled", kind: Kind::Pink },
    Unmapped { param: "sampled_pattern", kind: Kind::Pink },
    Unmapped { param: "texture", kind: Kind::Pink },
    Unmapped { param: "protect_texture", kind: Kind::Pink },
    Unmapped { param: "noise", kind: Kind::Pink },
    Unmapped { param: "grain", kind: Kind::Pink },
    // --- dynamics with no ProCraft counterpart (its Shape has scatter/jitter only) ---
    Unmapped { param: "hardness_dynamic", kind: Kind::Pink },
    Unmapped { param: "shape_dynamics", kind: Kind::Pink },
    Unmapped { param: "scattering", kind: Kind::Pink },
    Unmapped { param: "dual_brush", kind: Kind::Pink },
    Unmapped { param: "color_dynamics", kind: Kind::Pink },
    Unmapped { param: "transfer", kind: Kind::Pink },
    Unmapped { param: "pose", kind: Kind::Pink },
    Unmapped { param: "build_up", kind: Kind::Pink },
    Unmapped { param: "build_up_rate", kind: Kind::Pink },
    Unmapped { param: "motion", kind: Kind::Pink },
    Unmapped { param: "inertia", kind: Kind::Pink },
    Unmapped { param: "color_attributes", kind: Kind::Pink },
    // --- tip maths: ProCraft draws a round tip with `hardness`, or a shape image ---
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
    Unmapped { param: "taper", kind: Kind::Tool },
    Unmapped { param: "pressure_flow", kind: Kind::Tool },
];

/// Whether a field is pink (ProCraft's brush has no counterpart for it).
pub fn is_pink(param: &str) -> bool {
    UNMAPPED.iter().any(|u| u.param == param && u.kind == Kind::Pink)
}

/// The stamp diameter a stroke of `settings` is drawn at, canvas pixels.
pub fn diameter(settings: &BrushSettings) -> f32 {
    settings.size.clamp(0.5, 4096.0)
}

/// `settings` as ProCraft's brush.
///
/// Fields ProCraft has no counterpart for are left at ProCraft's own defaults — the pink list says
/// which; nothing here tries to imitate them, so what the canvas shows is ProCraft's brush and not
/// a blend of the two.
pub fn brush(settings: &BrushSettings) -> Brush {
    let mut brush = Brush::round_pen();
    brush.name = "PhotoCraft".to_owned();

    // Dabs: spacing is the distance between stamps as a fraction of the diameter, the same units
    // both sides use.
    brush.spacing = settings.spacing.clamp(0.001, 10.0);
    brush.hardness = settings.hardness.clamp(0.0, 1.0);
    brush.flow = settings.flow.clamp(0.0, 1.0);

    // Size and opacity: ProCraft's brush keeps the slider's range, and the stroke is built at a
    // diameter between them. One size, no range.
    brush.min_size = diameter(settings);
    brush.max_size = diameter(settings);
    brush.min_opacity = settings.opacity.clamp(0.0, 1.0);
    brush.max_opacity = settings.opacity.clamp(0.0, 1.0);

    // Pressure: our three switches are ProCraft's three bleeds at full strength.
    brush.pressure_size = if settings.pressure_size { 1.0 } else { 0.0 };
    brush.pressure_opacity = if settings.pressure_opacity { 1.0 } else { 0.0 };

    // The tip: roundness as drawn, fixed rotation (radians).
    brush.shape.roundness = settings.roundness.clamp(0.0, 1.0);
    brush.shape.rotation = settings.angle.to_radians();

    // Smoothing: our string length (0..1 of 100 px) is ProCraft's streamline (0..1).
    brush.streamline = if settings.smoothing.enabled { settings.smoothing.amount.clamp(0.0, 1.0) } else { 0.0 };

    brush
}

/// The stamp diameter a stroke is built at, for `StrokeBuilder::new`.
pub fn stroke_diameter(settings: &BrushSettings) -> f32 {
    diameter(settings)
}

/// ProCraft's stamps, as the dabs this app's painter draws.
///
/// The dab carries what ProCraft's stamp carries — where the stamp is, how big, how strong, how it
/// turns and how squashed — and what the stroke says for all of them: the brush's hardness, the
/// stroke's opacity ceiling and its colour. Everything our pigment/mixing/wet machinery would have
/// put on a dab of its own is left at the plain values (`Dab::round`), which is what the pink
/// controls' absence means: ProCraft's brush does not mix, it lays paint.
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
        assert_eq!(stroke_diameter(&settings), 900.0);
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
    fn a_stamp_becomes_a_dab_of_the_same_place_and_strength() {
        use procraft_engine::geom::Vec2;
        use procraft_engine::input::{Pointer, Sample};
        use procraft_engine::stroke::StrokeBuilder;

        let mut settings = BrushSettings::default();
        settings.size = 100.0;
        settings.spacing = 0.25;
        settings.opacity = 0.5;
        settings.flow = 1.0;
        settings.hardness = 0.8;
        settings.smoothing.amount = 0.0;

        let brush = brush(&settings);
        let mut builder = StrokeBuilder::new(brush, stroke_diameter(&settings), Pointer::Stylus);
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
    fn a_stroke_makes_stamps_at_the_settings_spacing() {
        use procraft_engine::geom::Vec2;
        use procraft_engine::input::{Pointer, Sample};
        use procraft_engine::stroke::StrokeBuilder;

        let mut settings = BrushSettings::default();
        settings.size = 100.0;
        settings.spacing = 0.05;
        settings.opacity = 1.0;
        settings.flow = 1.0;
        settings.smoothing.amount = 0.0;

        let brush = brush(&settings);
        let mut builder = StrokeBuilder::new(brush, stroke_diameter(&settings), Pointer::Stylus);
        let mut made = Vec::new();
        for i in 0..=40 {
            let x = i as f32 * 5.0;
            made.extend(builder.add(&[Sample::at(Vec2::new(x, 0.0), 1.0, i as f64 * 8.0)]));
        }
        assert!(!made.is_empty(), "a 200 px stroke at 5 % spacing must stamp");
        // 200 px of path, 5 px between stamps: about 40 stamps; the taper and the stroke's first
        // and last samples trim a few either way.
        assert!(made.len() >= 20, "stamps: {}", made.len());
        assert!(made.windows(2).all(|w| w[1].center.x >= w[0].center.x), "stamps run along the path");
    }
}

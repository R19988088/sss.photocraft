//! Halftone screening (网点 / comic screentone) for [`Adjustment::Halftone`].
//!
//! Every pixel picks its dot from the tone at its own document position: a screen cell grid is
//! rotated by the screen angle, the tone inside the cell becomes the dot's area, and the dot's
//! shape decides how that area sits around the cell's centre. Nothing reads a neighbour, so the
//! pass costs one cell lookup and one shape distance per channel and the same formula runs on
//! the CPU here and in the GPU's `halftone` kernel (`crates/gpu/src/compose.wgsl`), which is what
//! keeps the two renders identical to the last level.
//!
//! [`Adjustment::Halftone`]: photocraft_doc::Adjustment::Halftone

use photocraft_doc::adjust::HalftoneShape;

/// Channels at or above this (8-bit 252) are left alone with Preserve White, so paper stays paper.
pub const HALFTONE_WHITE_MIN: f32 = 252.0 / 255.0;

/// π as the shader spells it: both sides multiply by the same literal, so no ulp of difference.
const PI: f32 = 3.141_592_7;
/// 1/√2 for the diamond's distance.
const FRAC_1_SQRT_2: f32 = 0.707_106_78;
/// The smallest dot area the shapes are evaluated at, so an empty cell never divides by zero.
const MIN_AREA: f32 = 1e-4;

/// Screen angles added to the layer's own angle for the red, green and blue screens (`color`):
/// a four-colour set's 15°/75°/0° with blue on the base angle.
const SCREEN_OFFSETS: [f32; 3] = [15.0, 75.0, 0.0];

/// The ink fraction at which the shape reaches its cell's edges and the screen turns into the
/// paper between the dots: a circle touches at π/4, a diamond and the ellipse at half their cell.
/// A square, a line screen and a cross already tile their cell, so they never need a paper screen.
fn merge_at(shape: HalftoneShape) -> f32 {
    match shape {
        HalftoneShape::Round => 0.785_398_2,
        HalftoneShape::Diamond | HalftoneShape::Ellipse => 0.5,
        HalftoneShape::Square | HalftoneShape::Cross | HalftoneShape::Line => 1.0,
    }
}

/// A dot screen: the parameters of [`Adjustment::Halftone`] folded into what every pixel needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HalftoneScreen {
    /// Screen period in pixels (the cell's side).
    pub period: f32,
    /// (cos, sin) of the red, green and blue screens' angles; blue's is the layer's own angle.
    pub axes: [(f32, f32); 3],
    /// Ink gain from the Density percentage: 1 at 50%, 0 at 0%, 2 at 100%.
    pub gain: f32,
    /// Contrast as a factor on the tone about mid-grey: `1 + contrast/100`.
    pub contrast: f32,
    /// Brightness as an offset on the tone: `brightness/200`.
    pub brightness: f32,
    /// Cell-centre wobble as a fraction of half a cell (0..=1).
    pub jitter: f32,
    pub shape: HalftoneShape,
    /// One screen per channel instead of one screen on the grey tone.
    pub color: bool,
    /// Leave near-white pixels untouched.
    pub preserve_white: bool,
    /// White tolerance (0..=1 of the tone range): tones at or above `1 - white` print as paper.
    pub white: f32,
    /// Black tolerance (0..=1 of the tone range): tones at or below `black` print as solid ink.
    pub black: f32,
}

impl HalftoneScreen {
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        size: f32,
        angle: f32,
        density: f32,
        contrast: f32,
        brightness: f32,
        jitter: f32,
        shape: HalftoneShape,
        color: bool,
        preserve_white: bool,
        white: f32,
        black: f32,
    ) -> Self {
        let period = size.clamp(1.0, 512.0);
        let base = angle.to_radians();
        let axes = SCREEN_OFFSETS.map(|deg| {
            let a = base + deg.to_radians();
            (a.cos(), a.sin())
        });
        Self {
            period,
            axes,
            gain: density.clamp(0.0, 100.0) / 50.0,
            contrast: contrast.clamp(-100.0, 100.0) / 100.0,
            brightness: brightness.clamp(-100.0, 100.0) / 200.0,
            jitter: jitter.clamp(0.0, 100.0) / 100.0,
            shape,
            color,
            preserve_white,
            white: white.clamp(0.0, 100.0) / 100.0,
            black: black.clamp(0.0, 100.0) / 100.0,
        }
    }

    /// The printed colour at document pixel `(x, y)` for the tone `c`.
    ///
    /// Each channel prints as paper minus its dot's coverage, so dark tone turns into ink and
    /// light tone leaves the paper. In colour each channel keeps its own screen angle; in mono
    /// all three channels share one screen on the grey tone (black dots on white).
    pub fn eval(&self, c: [f32; 3], x: i32, y: i32) -> [f32; 3] {
        if self.color {
            [1.0 - self.dot(c[0], x, y, 0), 1.0 - self.dot(c[1], x, y, 1), 1.0 - self.dot(c[2], x, y, 2)]
        } else {
            // The W3C luminosity, as the Gradient Map's tone and Photoshop's grey screens use.
            let tone = 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
            [1.0 - self.dot(tone, x, y, 2); 3]
        }
    }

    /// The dot's coverage at `(x, y)` for one screen's `tone`.
    fn dot(&self, tone: f32, x: i32, y: i32, axis: usize) -> f32 {
        let period = self.period;
        let (cos, sin) = self.axes[axis.min(2)];
        // The cell grid, rotated about the canvas origin: exact document coordinates keep dots
        // from jumping when the canvas is panned or a tile is re-rendered.
        let u = x as f32 * cos + y as f32 * sin;
        let v = y as f32 * cos - x as f32 * sin;
        let iu = (u / period).floor();
        let iv = (v / period).floor();
        let fu = u - iu * period;
        let fv = v - iv * period;
        let cell = (iu as i32, iv as i32);
        // A hand-drawn screen: the cell's dot sits a little off-centre, hashed per cell. The hash
        // runs whatever the amount so multiplying by 0 keeps the centre exactly centred.
        let ox = (cell_noise(cell.0, cell.1) - 0.5) * self.jitter * period;
        let oy = (cell_noise(cell.0 + 7919, cell.1 + 104_729) - 0.5) * self.jitter * period;
        let dx = fu - 0.5 * period - ox;
        let dy = fv - 0.5 * period - oy;
        // Tone → ink: brightness and contrast act on the tone, density as a gain on the ink.
        let mut t = ((tone - 0.5) * (1.0 + self.contrast) + 0.5 + self.brightness).clamp(0.0, 1.0);
        // White and black tolerance: the ends of the tone range print as pure paper and pure ink
        // instead of as vanishingly small or nearly closed dots, so highlights and shadows stay
        // clean. White is tested first, as a tone can only be clipped from one end.
        if t >= 1.0 - self.white {
            t = 1.0;
        } else if t <= self.black {
            t = 0.0;
        }
        let area = ((1.0 - t) * self.gain).clamp(0.0, 1.0);
        let cell_area = period * period;
        let (ax, ay) = (dx.abs(), dy.abs());
        // The ink dot grows from the cell's centre until the shape reaches the cell's edges; a
        // further tone is the paper the dots leave between them, as a printed screen works.
        if area <= merge_at(self.shape) {
            let sd = self.shape_distance(dx, dy, area.max(MIN_AREA), period);
            // One pixel of antialiasing across the dot's edge, so dots and line screens stay
            // smooth at any angle without supersampling. A dot smaller than a pixel covers no
            // more than its own area (πr²), which keeps near-white paper clean.
            return (0.5 - sd).clamp(0.0, 1.0).min(area * cell_area);
        }
        // The paper between four merged dots: the same screen, inverted, at the cell's corners.
        // At full ink there is no paper at all, so black prints solid (not a grid of pinholes).
        let paper = 1.0 - area;
        let r = period * (paper / PI).sqrt();
        let (cx, cy) = (ax - 0.5 * period, ay - 0.5 * period);
        let hole = (0.5 - ((cx * cx + cy * cy).sqrt() - r)).clamp(0.0, 1.0).min(paper * cell_area);
        1.0 - hole
    }

    /// The shape's signed distance from `(dx, dy)` to its edge, in pixels: negative inside the
    /// dot. Every shape is sized so that it covers `area` of its cell.
    fn shape_distance(&self, dx: f32, dy: f32, area: f32, period: f32) -> f32 {
        let (ax, ay) = (dx.abs(), dy.abs());
        match self.shape {
            // Radius for the area: πr² = area·p².
            HalftoneShape::Round => (dx * dx + dy * dy).sqrt() - period * (area / PI).sqrt(),
            // Half diagonal of a diamond whose area 2h² is area·p².
            HalftoneShape::Diamond => (ax + ay - period * (area * 0.5).sqrt()) * FRAC_1_SQRT_2,
            // An ellipse with the circle's area, 1.4 as wide as it is tall.
            HalftoneShape::Ellipse => {
                let r = period * (area / PI).sqrt();
                let (a, b) = (r * 1.4, r * 0.7);
                (((dx * dx) / (a * a) + (dy * dy) / (b * b)).sqrt() - 1.0) * b
            }
            // Half side of a square whose area (2s)² is area·p².
            HalftoneShape::Square => ax.max(ay) - period * area.sqrt() * 0.5,
            // Two crossing bars of width w, area 2wp - w²: w/p = 1 - √(1 - area).
            HalftoneShape::Cross => ax.min(ay) - period * (1.0 - (1.0 - area).sqrt()) * 0.5,
            // A line screen: bars of width w = area·p across every cell.
            HalftoneShape::Line => ay - period * area * 0.5,
        }
    }

    /// The screen as the GPU kernel's four parameter records (`crates/gpu/src/plan/lower.rs`):
    /// `p0 = (period, blue cos, blue sin, gain)`, `p1 = (red cos, red sin, green cos, green sin)`,
    /// `p2 = (contrast, brightness, jitter, shape)`, `p3 = (flags, white, black, 0)` where the
    /// flags carry color (bit 0) and Preserve White (bit 1). Preserve White's threshold is the
    /// named constant both sides spell `252.0 / 255.0`, so no parameter slot is spent on it.
    pub fn program_params(&self) -> [[f32; 4]; 4] {
        let blue = self.axes[2];
        let red = self.axes[0];
        let green = self.axes[1];
        [
            [self.period, blue.0, blue.1, self.gain],
            [red.0, red.1, green.0, green.1],
            [self.contrast, self.brightness, self.jitter, self.shape.index() as f32],
            [(if self.color { 1.0 } else { 0.0 }) + (if self.preserve_white { 2.0 } else { 0.0 }), self.white, self.black, 0.0],
        ]
    }
}

/// The per-cell hash the jitter offsets come from: the shader's `halftone_noise`, bit for bit.
pub fn cell_noise(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ 0x9e37_79b9;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65536.0
}

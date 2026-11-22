//! 颜料混色: the pigment table paint mixes by (as Prefs' pigment mixing strength says) — `PM_lut.bin`
//! as the GPU's own 3D texture, ten bands of Kubelka-Munk response over a colour cube — and the
//! strength itself, which every canvas reads as it commits a stroke.

use std::sync::atomic::{AtomicU32, Ordering};

/// How strongly paint mixes as a pigment to start with: 90%, until the setting says otherwise.
pub const START: f32 = 0.9;

/// The table's shape: 32 × 32 points to a band's 32 slices, ten bands stacked on the depth axis.
pub(crate) const WIDTH: u32 = 32;
pub(crate) const HEIGHT: u32 = 32;
pub(crate) const DEPTH: u32 = 320;

/// Bytes the table holds: four half floats to a point (so the file is read with no decoding at all).
pub(crate) const BYTES: usize = (WIDTH * HEIGHT * DEPTH * 4 * 2) as usize;

/// The strength paint mixes at now (0..1).
fn strength_at() -> f32 {
    f32::from_bits(MIX.load(Ordering::Relaxed))
}

/// 0.9's bits: the strength every canvas starts at.
static MIX: AtomicU32 = AtomicU32::new(0x3F66_6666);

/// How strongly paint mixes with the colour under it as a pigment (0..1).
pub fn mix() -> f32 {
    strength_at()
}

/// The strength the passes that mix without a flag of their own read: [`mix`] where the table is in
/// the build, 0 where it isn't (so they mix plainly and need no flag).
pub(crate) fn strength() -> f32 {
    if bytes().is_some() { mix() } else { 0.0 }
}

/// The strength `set_mix` keeps of what it is given: inside 0..1, else [`START`].
fn kept(strength: f32) -> f32 {
    if strength.is_finite() { strength.clamp(0.0, 1.0) } else { START }
}

/// Sets how strongly paint mixes with the colour under it as a pigment (0..1; anything else keeps
/// it at [`START`]).
pub fn set_mix(strength: f32) {
    MIX.store(kept(strength).to_bits(), Ordering::Relaxed);
}

/// `PM_lut.bin` as it is uploaded: half floats the shader reads with the hardware's own filtering,
/// so neither side interpolates the cube by hand.
pub(crate) fn bytes() -> Option<&'static [u8]> {
    let bits: &[u8] = include_bytes!("../assets/PM_lut.bin");
    if bits.len() != BYTES {
        log::warn!("the pigment table is {} bytes, not {BYTES}: pigment mixing stays off", bits.len());
        return None;
    }
    Some(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_the_bytes_of_the_texture_it_is_uploaded_to() {
        let bits = bytes().expect("the pigment table is the texture's shape");
        assert_eq!(bits.len(), BYTES);
    }

    #[test]
    fn the_strength_keeps_to_the_unit_and_nan_starts() {
        // what a strength is kept as, without touching the one the canvases read (tests run together)
        assert_eq!(kept(f32::NAN), START);
        assert_eq!(kept(-1.0), 0.0);
        assert_eq!(kept(4.0), 1.0);
        assert_eq!(kept(START), START);
        assert!((0.0..=1.0).contains(&mix()));
    }
}

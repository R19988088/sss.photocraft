//! A brush's values moved a little, so that a brush modelled on a familiar kind has values of its
//! own: each by 1% to 3% of itself, up or down, fixed by the brush's name and the value's key, so
//! the brush is the same in every build.

use crate::brush::Brush;

/// The FNV-1a hash of `text`.
pub fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// How much brush `name` moves its control `key`: a share of its value, 1% to 3% either way
/// (2.95%, so that kept to four figures it stays within 3%), the same every time.
pub fn moved(name: &str, key: &str) -> f32 {
    // splitmix64 of the two names' hashes
    let mut z = (fnv(name) ^ fnv(key).rotate_left(17)).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    let share = 0.01 + 0.0195 * ((z >> 40) as f32 / (1u64 << 24) as f32);
    if z & 1 == 0 { share } else { -share }
}

/// `value` moved by `by` (a share of it) to four figures, kept inside `low..high` (moved the
/// other way where it would leave them); none moves at none or at either end, where moving would
/// change what it does.
pub fn shifted(value: f32, by: f32, (low, high): (f32, f32)) -> f32 {
    let inside = |v: f32| v > low + 1e-6 && v < high - 1e-6;
    if value.abs() < 1e-6 || !inside(value) {
        return value;
    }
    let figures = |v: f32| {
        let scale = 10f32.powi(3 - v.abs().log10().floor() as i32);
        (v * scale).round() / scale
    };
    [value * (1.0 + by), value * (1.0 - by)].into_iter().map(figures).find(|&v| inside(v)).unwrap_or(value)
}

/// The range of control `key`, as a brush keeps it.
pub fn range(key: &str) -> (f32, f32) {
    let mut b = Brush::round_pen();
    b.set_setting(key, -1000.0);
    let low = b.setting(key);
    b.set_setting(key, 1000.0);
    (low, b.setting(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_move_is_one_to_three_hundredths_the_same_every_time_and_stays_in_range() {
        for (name, key) in [("6B Pencil", "pen_taper_start"), ("Monoline", "streamline"), ("Sable", "hover_pressure")] {
            let by = moved(name, key);
            assert!((0.01..=0.0295).contains(&by.abs()), "{name} {key}: {by}");
            assert_eq!(moved(name, key), by);
        }
        assert!((shifted(0.5, 0.02, (0.0, 1.0)) - 0.51).abs() < 1e-6);
        // none and either end stay; past the end it moves the other way
        assert_eq!(shifted(0.0, 0.02, (0.0, 1.0)), 0.0);
        assert_eq!(shifted(1.0, -0.02, (0.0, 1.0)), 1.0);
        assert!((shifted(0.99, 0.02, (0.0, 1.0)) - 0.9702).abs() < 1e-6);
        assert_eq!(range("pen_taper_start"), (0.0, 1.0));
    }
}

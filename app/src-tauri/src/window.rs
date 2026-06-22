//! First-run main-window sizing.
//!
//! The window-state plugin remembers the user's size and position across
//! launches. On the very first launch there is nothing to restore, so we pick a
//! sensible default: roughly 70% of the screen, snapped down to a standard 16:9
//! resolution so the window keeps the same aspect ratio as the captures it sits
//! beside and never looks oversized on a large monitor.

/// The window's minimum size, mirrored from `tauri.conf.json`. The first-run
/// size never falls below this even on a small display.
const MIN_WIDTH: f64 = 880.0;
const MIN_HEIGHT: f64 = 600.0;

/// Fraction of the screen the first-run window aims to fill in each dimension.
const SCREEN_FRACTION: f64 = 0.7;

/// Standard 16:9 resolutions, ascending. The first-run size snaps down to the
/// largest of these that fits inside the target box.
const STANDARD_16_9: &[(f64, f64)] = &[
    (1280.0, 720.0),
    (1600.0, 900.0),
    (1920.0, 1080.0),
    (2560.0, 1440.0),
    (3840.0, 2160.0),
];

/// Pick the first-run window size for a screen of the given logical dimensions.
///
/// Aims for [`SCREEN_FRACTION`] of the screen in both dimensions, then snaps down
/// to the largest standard 16:9 resolution that fits. On a screen too small for
/// even the smallest standard resolution, it falls back to a 16:9 box fitted to
/// the target, clamped up to the window minimum.
pub fn pick_window_size(screen_w: f64, screen_h: f64) -> (f64, f64) {
    let target_w = screen_w * SCREEN_FRACTION;
    let target_h = screen_h * SCREEN_FRACTION;

    // Largest standard 16:9 resolution that fits the target box in both axes.
    // The list is ascending, so the last match (searching from the back) is the
    // largest that fits.
    let standard = STANDARD_16_9
        .iter()
        .copied()
        .rfind(|&(w, h)| w <= target_w && h <= target_h);

    if let Some((w, h)) = standard {
        return (w, h);
    }

    // Too small for any standard resolution: fit a 16:9 box inside the target,
    // then clamp up to the window minimum so it is never unusably small.
    let w = target_w.min(target_h * 16.0 / 9.0);
    let h = w * 9.0 / 16.0;
    (w.max(MIN_WIDTH), h.max(MIN_HEIGHT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_down_to_standard_resolution_on_common_screens() {
        // 1080p: 70% is 1344x756, so 1280x720 is the largest that fits.
        assert_eq!(pick_window_size(1920.0, 1080.0), (1280.0, 720.0));
        // 1440p: 70% is 1792x1008, so 1600x900 fits but 1920x1080 does not.
        assert_eq!(pick_window_size(2560.0, 1440.0), (1600.0, 900.0));
        // 4K: 70% is 2688x1512, so 2560x1440 fits but 3840x2160 does not.
        assert_eq!(pick_window_size(3840.0, 2160.0), (2560.0, 1440.0));
    }

    #[test]
    fn chosen_size_is_a_standard_16_9_resolution_within_target() {
        let (w, h) = pick_window_size(2560.0, 1440.0);
        assert!(STANDARD_16_9.contains(&(w, h)));
        assert!(w <= 2560.0 * SCREEN_FRACTION && h <= 1440.0 * SCREEN_FRACTION);
    }

    #[test]
    fn small_screen_falls_back_to_fitted_box_at_minimum() {
        // A small laptop screen cannot fit even 1280x720 at 70%.
        let (w, h) = pick_window_size(1366.0, 768.0);
        assert!(w >= MIN_WIDTH && h >= MIN_HEIGHT);
        assert!(w <= 1366.0 && h <= 768.0);
        // Not one of the standard resolutions, since none fit.
        assert!(!STANDARD_16_9.contains(&(w, h)));
    }

    #[test]
    fn never_below_the_window_minimum() {
        let (w, h) = pick_window_size(640.0, 480.0);
        assert!(w >= MIN_WIDTH && h >= MIN_HEIGHT);
    }
}

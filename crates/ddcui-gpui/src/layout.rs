//! Window and level-bar geometry.
//!
//! A level bar is a fixed width inside a fixed-width card, so a click can be
//! turned into a percentage with arithmetic instead of asking the layout engine
//! where the bar ended up. That is also why the window has a minimum width: the
//! cards never have to shrink, so the arithmetic stays exact.

/// How wide and tall the window opens.
pub const WINDOW_WIDTH: f32 = 620.0;
pub const WINDOW_HEIGHT: f32 = 720.0;

/// How small the window may be dragged. Anything narrower would clip the cards.
pub const WINDOW_MIN_WIDTH: f32 = 600.0;
pub const WINDOW_MIN_HEIGHT: f32 = 460.0;

/// Space between the window edge and the cards.
///
/// The window is exactly one card plus two of these, so the cards are centred
/// with no slack on either side. That is what lets a click's position in the
/// window be turned into a bar position by arithmetic.
pub const PAGE_PADDING: f32 = (WINDOW_WIDTH - CARD_WIDTH) / 2.0;

/// Width of one monitor card, outline and padding included.
pub const CARD_WIDTH: f32 = 560.0;

/// Padding inside a card.
pub const CARD_PADDING: f32 = 12.0;

/// Width of a card's outline.
pub const CARD_BORDER: f32 = 1.0;

/// Left edge of every level bar, relative to the window.
pub const TRACK_LEFT: f32 = PAGE_PADDING + CARD_BORDER + CARD_PADDING;

/// Width of a level bar: the card minus its outline and padding.
pub const TRACK_WIDTH: f32 = CARD_WIDTH - 2.0 * (CARD_BORDER + CARD_PADDING);

/// Height of the bar itself, and of the area that accepts clicks around it.
pub const BAR_HEIGHT: f32 = 8.0;
pub const TRACK_HEIGHT: f32 = 24.0;

/// Size of the marks drawn on a level bar.
pub const TICK_WIDTH: f32 = 2.0;
pub const TICK_HEIGHT: f32 = 4.0;

/// The percentages marked on a level bar.
pub const TICKS: [u8; 5] = [0, 25, 50, 75, 100];

/// The percentage at window x, clamped to the ends of the bar.
pub fn percent_at(x: f32) -> u8 {
    let fraction = ((x - TRACK_LEFT) / TRACK_WIDTH).clamp(0.0, 1.0);
    (fraction * 100.0).round() as u8
}

/// How much of the bar is filled for `percent`.
pub fn fill_width(percent: u8) -> f32 {
    f32::from(percent.min(100)) / 100.0 * TRACK_WIDTH
}

/// Where tick `index` of [`TICKS`] is drawn, from the left of the track.
///
/// Ticks are nudged inside the ends so the outermost ones stay visible.
pub fn tick_offset(index: usize) -> f32 {
    let last = (TICKS.len() - 1) as f32;
    (index as f32 / last) * (TRACK_WIDTH - TICK_WIDTH)
}

// A card has to fit inside the smallest window the window manager allows,
// otherwise the click arithmetic above would be wrong about where the bars are.
// The default window fits one card and two page paddings exactly, so the cards
// are centred when it opens.
const _: () = assert!(TRACK_LEFT + TRACK_WIDTH <= WINDOW_MIN_WIDTH);
const _: () = assert!(CARD_WIDTH + 2.0 * PAGE_PADDING == WINDOW_WIDTH);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_maps_to_the_percentage_under_it() {
        assert_eq!(percent_at(TRACK_LEFT), 0);
        assert_eq!(percent_at(TRACK_LEFT + TRACK_WIDTH / 2.0), 50);
        assert_eq!(percent_at(TRACK_LEFT + TRACK_WIDTH), 100);
        // A quarter of the way in is a quarter of the range.
        assert_eq!(percent_at(TRACK_LEFT + TRACK_WIDTH / 4.0), 25);
    }

    #[test]
    fn clicks_outside_the_bar_are_clamped() {
        assert_eq!(percent_at(0.0), 0);
        assert_eq!(percent_at(TRACK_LEFT - 40.0), 0);
        assert_eq!(percent_at(TRACK_LEFT + TRACK_WIDTH + 40.0), 100);
        assert_eq!(percent_at(f32::MAX), 100);
    }

    #[test]
    fn the_fill_tracks_the_percentage() {
        assert_eq!(fill_width(0), 0.0);
        assert_eq!(fill_width(100), TRACK_WIDTH);
        assert_eq!(fill_width(200), TRACK_WIDTH);
        assert!(fill_width(25) < fill_width(50));
    }

    #[test]
    fn ticks_stay_inside_the_bar() {
        for index in 0..TICKS.len() {
            let left = tick_offset(index);
            assert!(left >= 0.0, "tick {index} at {left}");
            assert!(
                left + TICK_WIDTH <= TRACK_WIDTH + 0.01,
                "tick {index} at {left}"
            );
        }
        assert!(tick_offset(0) < tick_offset(1));
        assert_eq!(TICKS, [0, 25, 50, 75, 100]);
    }
}

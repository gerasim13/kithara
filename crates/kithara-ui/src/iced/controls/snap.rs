use iced::Rectangle;

/// Snaps both control edges to the pixel grid used by Masonry, so the immediate
/// and retained hosts pass the same box to their painter. Rounding width alone
/// is different: 262.5 covers 263 pixels at an integer origin but 262 at a half
/// origin. A differing waveform width changes every column's source slice.
pub(crate) fn snapped(bounds: Rectangle) -> Rectangle {
    let x = bounds.x.round();
    let y = bounds.y.round();
    Rectangle {
        height: (bounds.y + bounds.height).round() - y,
        width: (bounds.x + bounds.width).round() - x,
        x,
        y,
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    /// The four boxes the stress page's wave row is split into: 1050 points
    /// over four columns and three gaps, so two of the four start on a half
    /// pixel. This is the case the two hosts drew differently.
    fn wave_row() -> Vec<Rectangle> {
        [0.0_f32, 1.0, 2.0, 3.0]
            .into_iter()
            .map(|index| Rectangle {
                height: 120.0,
                width: 262.5,
                x: 212.0 + index * (262.5 + 8.0),
                y: 85.0,
            })
            .collect()
    }

    /// What Masonry hands the retained painter, spelled out here rather than
    /// borrowed from the function under test, so the two cannot drift together.
    fn masonry_size(bounds: Rectangle) -> (f32, f32) {
        (
            (bounds.x + bounds.width).round() - bounds.x.round(),
            (bounds.y + bounds.height).round() - bounds.y.round(),
        )
    }

    #[kithara::test]
    fn the_immediate_box_is_sized_like_the_retained_one() {
        for bounds in wave_row() {
            let snapped = snapped(bounds);

            assert_eq!(
                (snapped.width, snapped.height),
                masonry_size(bounds),
                "box at {} was sized unlike the retained host's node",
                bounds.x
            );
        }
    }

    /// The defect this exists for: rounding the width reads 263 for every box
    /// in the row, while the row really covers 263, 262, 263, 262 pixels.
    #[kithara::test]
    fn a_half_pixel_origin_costs_the_box_a_pixel() {
        let widths: Vec<f32> = wave_row()
            .into_iter()
            .map(|bounds| snapped(bounds).width)
            .collect();

        assert_eq!(widths, [263.0, 262.0, 263.0, 262.0]);
    }

    /// A box already on the grid is left where it is.
    #[kithara::test]
    fn a_whole_pixel_box_is_left_alone() {
        let bounds = Rectangle {
            height: 40.0,
            width: 200.0,
            x: 12.0,
            y: 8.0,
        };

        assert_eq!(snapped(bounds), bounds);
    }
}

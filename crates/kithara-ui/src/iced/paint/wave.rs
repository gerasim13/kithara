use std::ops::Range;

use crate::{
    atoms::wave::{
        face::{Drawn, Wave},
        overlay,
        paint::WavePaint,
    },
    draw::Rect,
    module::WaveStyle,
};

impl Wave {
    pub(crate) const fn hero(&self) -> bool {
        matches!(self.style, WaveStyle::Hero)
    }

    /// Where the naming panel sits, so a host can tell whether the pointer is
    /// on it.
    pub(crate) fn overlay_bounds(&self, data: &Drawn, bounds: Rect) -> Rect {
        self.face(data).overlay_bounds(bounds)
    }
}

impl Drawn {
    pub(crate) fn has_waveform(&self) -> bool {
        self.waveform
            .as_ref()
            .is_some_and(|waveform| !waveform.buckets.is_empty())
    }
}

impl WavePaint<'_> {
    pub(crate) fn overlay_bounds(&self, bounds: Rect) -> Rect {
        overlay::strip(bounds, self.metrics.overlay)
    }
}

pub(crate) fn x_to_norm(x: f32, window: &Range<f32>, width: f32) -> Option<f32> {
    (width > 0.0).then(|| {
        (x / width)
            .mul_add(window.end - window.start, window.start)
            .clamp(0.0, 1.0)
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::x_to_norm;
    use crate::{
        atoms::wave::{
            bars::{CoverageSpan, coverage_spans},
            face::tests::hero,
            zoom_math::{norm_to_x, window_bounds},
        },
        builtin,
        draw::{Pt, Rect},
        render::{DEFAULT_ZOOM, Zoom},
    };

    #[kithara::test]
    fn pointer_positions_map_back_through_the_zoom_window() {
        let window = window_bounds(0.5, 0.2);

        assert_eq!(x_to_norm(0.0, &window, 200.0), Some(0.4));
        assert_eq!(x_to_norm(100.0, &window, 200.0), Some(0.5));
        assert_eq!(x_to_norm(200.0, &window, 200.0), Some(0.6));
        assert_eq!(x_to_norm(100.0, &window, 0.0), None);
    }

    #[kithara::test]
    fn pointer_positions_clamp_to_track_bounds() {
        let start = window_bounds(0.01, DEFAULT_ZOOM);
        let end = window_bounds(0.99, DEFAULT_ZOOM);

        assert_eq!(x_to_norm(0.0, &start, 200.0), Some(0.0));
        assert_eq!(x_to_norm(200.0, &end, 200.0), Some(1.0));
    }

    /// The single unready stretch a partition holds.
    fn marked(spans: impl Iterator<Item = CoverageSpan>) -> [f32; 2] {
        let mut marked = None;
        for span in spans {
            if let CoverageSpan::Unready(span, _) = span {
                assert!(marked.replace(span).is_none(), "only one hole is marked");
            }
        }
        marked.expect("one hole is marked")
    }

    fn assert_within(actual: [f32; 2], expected: [f32; 2], tolerance: f32) {
        let off = [
            (actual[0] - expected[0]).abs(),
            (actual[1] - expected[1]).abs(),
        ];
        assert!(
            off[0] <= tolerance && off[1] <= tolerance,
            "{actual:?} is not {expected:?} within {tolerance}"
        );
    }

    /// The deck window and the overview place a region on the same audio: each
    /// pixel span maps back to the fractions it was drawn from, outward by at
    /// most the pixel each side was rounded by.
    #[kithara::test]
    fn the_deck_and_the_overview_mark_the_same_track_positions() {
        const WIDTH: f32 = 400.0;
        let hole = [0.25, 0.5];
        let window = window_bounds(0.375, f32::from(Zoom::MAX));

        let deck = marked(coverage_spans(
            &[hole],
            |norm| norm_to_x(norm, &window, WIDTH),
            WIDTH,
        ));
        let overview = marked(coverage_spans(&[hole], |norm| norm * WIDTH, WIDTH));

        assert_within(
            deck.map(|x| x_to_norm(x, &window, WIDTH).expect("a positive width")),
            hole,
            f32::from(Zoom::MAX) / WIDTH,
        );
        assert_within(overview.map(|x| x / WIDTH), hole, 1.0 / WIDTH);
    }

    /// The panel steps aside for a pointer on it, not for one anywhere on the
    /// wave, so the box a host tests against has to be the panel's own.
    #[kithara::test]
    fn the_panel_covers_the_top_of_the_wave_and_no_more() {
        let skin = builtin::skin();
        let (painter, data) = hero(skin);
        let bounds = Rect {
            h: 300.0,
            w: 400.0,
            x: 100.0,
            y: 50.0,
        };
        let panel = painter.overlay_bounds(&data, bounds);

        assert!(panel.contains(Pt {
            x: 150.0,
            y: 50.0 + skin.wave.overlay.height / 2.0,
        }));
        assert!(!panel.contains(Pt {
            x: 150.0,
            y: 50.0 + skin.wave.overlay.height + 40.0,
        }));
    }

    /// And on a wave shorter than the panel it stops at the wave, rather than
    /// hanging below it.
    #[kithara::test]
    fn the_panel_clamps_to_a_short_wave() {
        let skin = builtin::skin();
        let (painter, data) = hero(skin);
        let bounds = Rect {
            h: skin.wave.overlay.height / 2.0,
            w: 200.0,
            x: 0.0,
            y: 0.0,
        };

        assert_eq!(painter.overlay_bounds(&data, bounds).h, bounds.h);
    }
}

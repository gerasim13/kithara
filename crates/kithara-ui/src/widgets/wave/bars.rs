use std::iter;

use iced::{Color, Point, Rectangle, Size, widget::canvas::Frame};

use crate::{
    render::{WaveBucket, theme::RenderPalette},
    skin::WaveSkin,
};

/// Column pitch: one bar plus the gap after it.
pub(crate) fn step(metrics: WaveSkin) -> f32 {
    metrics.bar_width + metrics.bar_gap
}

/// Dim everything left of the playhead. `played_x` is the playhead in canvas
/// pixels, so callers own the norm-to-pixel mapping their window implies.
pub(crate) fn draw_played(
    frame: &mut Frame,
    bounds: Rectangle,
    played_x: f32,
    alpha: f32,
    palette: RenderPalette,
) {
    frame.fill_rectangle(
        Point::ORIGIN,
        Size::new(played_x.clamp(0.0, bounds.width), bounds.height),
        Color {
            a: alpha,
            ..palette.bg_deep
        },
    );
}

/// One column of the waveform: the three bands share a width and nest by
/// level, each drawn from the vertical centre over the previous one.
pub(crate) fn draw_column(
    frame: &mut Frame,
    bounds: Rectangle,
    center_x: f32,
    bucket: WaveBucket,
    available_height: f32,
    metrics: WaveSkin,
    palette: RenderPalette,
) {
    for (level, color) in [
        (bucket.low, palette.wave_low),
        (bucket.mid, palette.wave_mid),
        (bucket.high, palette.wave_high),
    ] {
        let height = level.clamp(0.0, 1.0) * available_height;
        if height <= 0.0 {
            continue;
        }
        frame.fill_rectangle(
            Point::new(
                center_x - metrics.bar_width / 2.0,
                (bounds.height - height) / 2.0,
            ),
            Size::new(metrics.bar_width, height),
            color,
        );
    }
}

/// Colors the coverage layer resolves from the skin once per frame.
#[derive(Clone, Copy)]
pub(crate) struct CoveragePalette {
    /// Baseline and comb stubs.
    pub(crate) mark: Color,
    /// Rails and region boundaries.
    pub(crate) edge: Color,
}

/// One stretch of the lane, in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CoverageSpan {
    Covered([f32; 2]),
    /// An unready stretch, with the sides that meet covered audio.
    Unready([f32; 2], [bool; 2]),
}

/// Partition the lane into what the analysis covered and what it did not.
///
/// `to_x` maps a track fraction to a canvas pixel, so each caller keeps its own
/// window mapping. `unready` is in ascending track order. Every unready stretch
/// rounds outward, so a pixel column that is part covered reads unready: the
/// picture may understate what is known by under a pixel, never overstate it.
pub(crate) fn coverage_spans<'a, F>(
    unready: &'a [[f32; 2]],
    to_x: F,
    width: f32,
) -> impl Iterator<Item = CoverageSpan> + 'a
where
    F: Fn(f32) -> f32 + 'a,
{
    unready
        .iter()
        .copied()
        .map(Some)
        .chain(iter::once(None))
        .scan(0.0, move |covered_from, hole| {
            let Some([start, end]) = hole else {
                return Some([
                    (width > *covered_from)
                        .then_some(CoverageSpan::Covered([*covered_from, width])),
                    None,
                ]);
            };
            let x0 = to_x(start).floor().clamp(0.0, width);
            let x1 = to_x(end).ceil().clamp(0.0, width);
            if x1 <= x0 {
                return Some([None, None]);
            }
            let covered =
                (x0 > *covered_from).then_some(CoverageSpan::Covered([*covered_from, x0]));
            *covered_from = x1;
            Some([
                covered,
                Some(CoverageSpan::Unready([x0, x1], [start > 0.0, end < 1.0])),
            ])
        })
        .flatten()
        .flatten()
}

/// Draw what the analysis has covered and what it has not.
///
/// A covered stretch keeps a continuous baseline, so an unbroken axis with no
/// bars reads as analysed silence. An unready stretch gets rails, stubs at the
/// bar pitch, and a boundary only where it meets covered audio. Every part is
/// its own rectangle, so nothing is clipped and no gradient mesh is cut.
pub(crate) fn draw_coverage<F>(
    frame: &mut Frame,
    bounds: Rectangle,
    unready: &[[f32; 2]],
    to_x: F,
    metrics: WaveSkin,
    palette: CoveragePalette,
) where
    F: Fn(f32) -> f32,
{
    for span in coverage_spans(unready, to_x, bounds.width) {
        match span {
            CoverageSpan::Covered(span) => draw_baseline(frame, bounds, span, metrics, palette),
            CoverageSpan::Unready(span, boundaries) => {
                draw_unready(frame, bounds, span, boundaries, metrics, palette);
            }
        }
    }
}

/// The axis of a covered stretch, drawn whatever the bands read.
fn draw_baseline(
    frame: &mut Frame,
    bounds: Rectangle,
    span: [f32; 2],
    metrics: WaveSkin,
    palette: CoveragePalette,
) {
    let height = metrics.coverage_hairline;
    frame.fill_rectangle(
        Point::new(span[0], (bounds.height - height) / 2.0),
        Size::new(span[1] - span[0], height),
        Color {
            a: metrics.coverage_baseline_alpha,
            ..palette.mark
        },
    );
}

/// One unready region: edge rails, a stub per column, and a boundary on each
/// side that meets covered audio.
fn draw_unready(
    frame: &mut Frame,
    bounds: Rectangle,
    span: [f32; 2],
    boundaries: [bool; 2],
    metrics: WaveSkin,
    palette: CoveragePalette,
) {
    let width = span[1] - span[0];
    let rail = metrics.coverage_rail_height;
    frame.fill_rectangle(
        Point::new(span[0], 0.0),
        Size::new(width, rail),
        palette.edge,
    );
    frame.fill_rectangle(
        Point::new(span[0], bounds.height - rail),
        Size::new(width, rail),
        palette.edge,
    );

    let stub = metrics.coverage_stub_height;
    let stub_color = Color {
        a: metrics.coverage_stub_alpha,
        ..palette.mark
    };
    let top = (bounds.height - stub) / 2.0;
    for x in stub_positions(span, step(metrics)) {
        frame.fill_rectangle(
            Point::new(x, top),
            Size::new(metrics.bar_width.min(span[1] - x), stub),
            stub_color,
        );
    }

    let edge = metrics.coverage_hairline;
    for (present, x) in boundaries.into_iter().zip([span[0], span[1] - edge]) {
        if present {
            frame.fill_rectangle(
                Point::new(x, 0.0),
                Size::new(edge, bounds.height),
                palette.edge,
            );
        }
    }
}

/// Stub columns standing in for the bars a region has no data for, on the same
/// grid the real columns sit on so the two read as one lane.
fn stub_positions(span: [f32; 2], pitch: f32) -> impl Iterator<Item = f32> {
    let mut x = if pitch > 0.0 {
        (span[0] / pitch).ceil() * pitch
    } else {
        span[1]
    };
    iter::from_fn(move || {
        let at = (x < span[1]).then_some(x)?;
        x += pitch;
        Some(at)
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{
        super::zoom_math::{MAX_ZOOM, norm_to_x, window_bounds, x_to_norm},
        CoverageSpan, coverage_spans, stub_positions,
    };

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

    /// Nothing uncovered leaves one unbroken covered stretch: an axis with no
    /// bars is analysed silence, which is what keeps silence from reading as
    /// absence.
    #[kithara::test]
    fn a_fully_covered_lane_is_one_covered_span() {
        assert!(
            coverage_spans(&[], |norm| norm * 100.0, 100.0)
                .eq([CoverageSpan::Covered([0.0, 100.0])])
        );
    }

    /// A hole between two covered runs breaks the baseline exactly where the
    /// data stops, and takes a boundary on both sides it meets.
    #[kithara::test]
    fn a_hole_breaks_the_baseline_and_takes_both_boundaries() {
        assert!(
            coverage_spans(&[[0.25, 0.5]], |norm| norm * 100.0, 100.0).eq([
                CoverageSpan::Covered([0.0, 25.0]),
                CoverageSpan::Unready([25.0, 50.0], [true, true]),
                CoverageSpan::Covered([50.0, 100.0]),
            ])
        );
    }

    /// A region that runs to an end of the track has no covered audio to meet
    /// there, so that side takes no boundary.
    #[kithara::test]
    fn a_region_at_the_edge_drops_the_boundary_it_does_not_meet() {
        assert!(
            coverage_spans(&[[0.0, 0.25], [0.75, 1.0]], |norm| norm * 100.0, 100.0).eq([
                CoverageSpan::Unready([0.0, 25.0], [false, true]),
                CoverageSpan::Covered([25.0, 75.0]),
                CoverageSpan::Unready([75.0, 100.0], [true, false]),
            ])
        );
    }

    /// A gap that lands between pixels rounds outward, so a column that is
    /// part covered reads unready rather than claiming audio nothing decoded.
    #[kithara::test]
    fn a_gap_off_the_pixel_grid_rounds_outward() {
        assert!(
            coverage_spans(&[[0.404, 0.606]], |norm| norm * 100.0, 100.0).eq([
                CoverageSpan::Covered([0.0, 40.0]),
                CoverageSpan::Unready([40.0, 61.0], [true, true]),
                CoverageSpan::Covered([61.0, 100.0]),
            ])
        );
    }

    /// A region outside the window contributes nothing, so a zoomed view is
    /// not told about track it cannot show.
    #[kithara::test]
    fn a_region_outside_the_view_is_dropped() {
        assert!(
            coverage_spans(&[[0.0, 0.1]], |norm| (norm - 0.5) * 100.0, 100.0)
                .eq([CoverageSpan::Covered([0.0, 100.0])])
        );
    }

    /// The deck window and the overview place a region on the same audio: each
    /// pixel span maps back to the fractions it was drawn from, outward by at
    /// most the pixel each side was rounded by.
    #[kithara::test]
    fn the_deck_and_the_overview_mark_the_same_track_positions() {
        const WIDTH: f32 = 400.0;
        let hole = [0.25, 0.5];
        let window = window_bounds(0.375, MAX_ZOOM);

        let deck = marked(coverage_spans(
            &[hole],
            |norm| norm_to_x(norm, &window, WIDTH),
            WIDTH,
        ));
        let overview = marked(coverage_spans(&[hole], |norm| norm * WIDTH, WIDTH));

        assert_within(
            deck.map(|x| x_to_norm(x, &window, WIDTH).expect("a positive width")),
            hole,
            MAX_ZOOM / WIDTH,
        );
        assert_within(overview.map(|x| x / WIDTH), hole, 1.0 / WIDTH);
    }

    /// Stubs sit on the grid the real columns tile from, not on the region's
    /// own origin, so the two read as one lane.
    #[kithara::test]
    fn stubs_land_on_the_column_grid() {
        let stubs: Vec<f32> = stub_positions([5.0, 20.0], 4.0).collect();

        assert_eq!(stubs, [8.0, 12.0, 16.0]);
    }
}

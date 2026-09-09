use std::ops::Range;

use num_traits::cast::AsPrimitive;

use super::{
    bars,
    bars::Played,
    paint::WavePalette,
    zoom_math::{
        bar_bucket_range, bar_grid, max_bucket, norm_to_x, visible_mark_range, visible_marks,
        window_bounds,
    },
};
use crate::{
    atoms::design::quad::rule,
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    render::WaveBucket,
    shaping::TextContext,
    skin::WaveSkin,
};

#[derive(Clone, Copy)]
pub(crate) struct HeroPalette {
    pub(crate) cue_badge: Rgba,
    pub(crate) cue_text: Rgba,
    pub(crate) base: WavePalette,
}

#[derive(Clone, Copy)]
pub(crate) struct HeroWave<'a> {
    pub(crate) buckets: &'a [WaveBucket],
    pub(crate) unready: &'a [[f32; 2]],
    pub(crate) beats: &'a [f32],
    pub(crate) cues: &'a [f32],
    pub(crate) downbeats: &'a [f32],
    pub(crate) loop_region: Option<[f32; 2]>,
    pub(crate) position: f32,
    pub(crate) zoom: f32,
}

pub(crate) fn draw(
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    bounds: Rect,
    data: HeroWave<'_>,
    metrics: WaveSkin,
    palette: HeroPalette,
) {
    let window = window_bounds(data.position, data.zoom);
    draw_bars(list, bounds, data, &window, metrics, palette.base);
    bars::draw_coverage(
        list,
        bounds,
        data.unready,
        |norm| norm_to_x(norm, &window, bounds.w),
        metrics,
        palette.base.coverage,
    );
    draw_grid(list, bounds, data, &window, metrics, palette.base);
    if let Some(region) = data.loop_region {
        draw_loop(list, bounds, region, &window, metrics, palette.base);
    }
    draw_cues(list, text, bounds, data.cues, &window, metrics, palette);
    draw_playhead(list, bounds, data.position, &window, metrics, palette.base);
}

fn draw_bars(
    list: &mut DrawListBuilder,
    bounds: Rect,
    data: HeroWave<'_>,
    window: &Range<f32>,
    metrics: WaveSkin,
    palette: WavePalette,
) {
    let step = bars::step(metrics);
    let Some(grid) = bar_grid(bounds.w.round(), step, data.zoom, window) else {
        return;
    };
    let played = Played::new(
        bounds.x + norm_to_x(data.position.clamp(0.0, 1.0), window, bounds.w),
        metrics.played_alpha,
        palette.trough,
    );
    let available_height = (bounds.h - metrics.content_inset * 2.0).max(0.0);
    let origin_x = bounds.x - window.start / grid.norm_width * step;
    for bar in grid.first..grid.last {
        let range = bar_bucket_range(bar, grid.norm_width, data.buckets.len());
        let Some(bucket) = max_bucket(data.buckets, range) else {
            continue;
        };
        let bar_f: f32 = bar.as_();
        let center_x = (bar_f + 0.5).mul_add(step, origin_x);
        let colors = [palette.band_low, palette.band_mid, palette.band_high];
        bars::draw_column(
            list,
            bounds,
            center_x,
            bucket,
            available_height,
            metrics,
            played.colors(center_x, colors),
        );
    }
}

fn draw_grid(
    list: &mut DrawListBuilder,
    bounds: Rect,
    data: HeroWave<'_>,
    window: &Range<f32>,
    metrics: WaveSkin,
    palette: WavePalette,
) {
    draw_marks(
        list,
        bounds,
        visible_marks(data.beats, window),
        window,
        with_alpha(palette.grid, metrics.grid_alpha),
        metrics.grid_width,
    );
    draw_marks(
        list,
        bounds,
        visible_marks(data.downbeats, window),
        window,
        with_alpha(palette.label, metrics.downbeat_alpha),
        metrics.grid_width,
    );
}

fn draw_marks(
    list: &mut DrawListBuilder,
    bounds: Rect,
    marks: &[f32],
    window: &Range<f32>,
    color: Rgba,
    width: f32,
) {
    for &mark in marks {
        let x = bounds.x + norm_to_x(mark, window, bounds.w);
        rule(list, bounds, x, width, color);
    }
}

fn draw_loop(
    list: &mut DrawListBuilder,
    bounds: Rect,
    region: [f32; 2],
    window: &Range<f32>,
    metrics: WaveSkin,
    palette: WavePalette,
) {
    let start = region[0].min(region[1]).clamp(0.0, 1.0);
    let end = region[0].max(region[1]).clamp(0.0, 1.0);
    let start_x = norm_to_x(start, window, bounds.w);
    let end_x = norm_to_x(end, window, bounds.w);
    let visible_start = start_x.clamp(0.0, bounds.w);
    let visible_end = end_x.clamp(0.0, bounds.w);
    if visible_end > visible_start {
        list.fill_rect(
            Rect {
                h: bounds.h,
                w: visible_end - visible_start,
                x: bounds.x + visible_start,
                y: bounds.y,
            },
            with_alpha(palette.played, metrics.loop_fill_alpha),
        );
    }
    for x in [start_x, end_x] {
        if (0.0..=bounds.w).contains(&x) {
            let x = bounds.x + x;
            rule(list, bounds, x, metrics.loop_bound_width, palette.played);
        }
    }
}

fn draw_cues(
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    bounds: Rect,
    cues: &[f32],
    window: &Range<f32>,
    metrics: WaveSkin,
    palette: HeroPalette,
) {
    for index in visible_mark_range(cues, window) {
        let x = bounds.x + norm_to_x(cues[index], window, bounds.w);
        rule(
            list,
            bounds,
            x,
            metrics.cue_line_width,
            palette.base.band_high,
        );
        let badge = Rect {
            h: metrics.cue_badge_size,
            w: metrics.cue_badge_size,
            x: x - metrics.cue_badge_size / 2.0,
            y: bounds.y,
        };
        list.fill_rect(badge, palette.cue_badge);
        draw_cue_text(list, text, badge, index + 1, metrics, palette.cue_text);
    }
}

fn draw_cue_text(
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    bounds: Rect,
    index: usize,
    metrics: WaveSkin,
    color: Rgba,
) {
    let content = index.to_string();
    let run = text.shape(&content, metrics.cue_badge_text, Some(bounds.w));
    list.text(
        &run,
        &content,
        Transform::translate(Pt {
            x: bounds.x + (bounds.w - run.width()) / 2.0,
            y: bounds.y + (bounds.h - run.height()) / 2.0,
        }),
        color,
    );
}

fn draw_playhead(
    list: &mut DrawListBuilder,
    bounds: Rect,
    position: f32,
    window: &Range<f32>,
    metrics: WaveSkin,
    palette: WavePalette,
) {
    let x = bounds.x + norm_to_x(position.clamp(0.0, 1.0), window, bounds.w);
    rule(list, bounds, x, metrics.playhead_width, palette.played);
    let marker_x = x - metrics.playhead_marker_width / 2.0;
    for y in [
        bounds.y,
        bounds.y + (bounds.h - metrics.playhead_marker_height).max(0.0),
    ] {
        list.fill_rect(
            Rect {
                y,
                h: metrics.playhead_marker_height,
                w: metrics.playhead_marker_width,
                x: marker_x,
            },
            palette.played,
        );
    }
}

const fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        atoms::wave::zoom_math::DEFAULT_ZOOM,
        builtin,
        draw::{DrawCmd, Geom},
    };

    fn palette() -> WavePalette {
        let ink = Rgba {
            a: 1.0,
            b: 1.0,
            g: 1.0,
            r: 1.0,
        };
        WavePalette {
            coverage: bars::CoveragePalette {
                edge: ink,
                mark: ink,
            },
            trough: ink,
            grid: ink,
            label: ink,
            played: ink,
            band_low: ink,
            band_mid: ink,
            band_high: ink,
        }
    }

    /// The left edge of every column the hero wave draws, in order.
    fn column_edges(bounds: Rect, zoom: f32) -> Vec<f32> {
        let buckets = (0..4096)
            .map(|index| {
                let level: f32 = index.as_();
                let level = 0.3 + (level * 0.017).sin().abs() * 0.6;
                WaveBucket {
                    high: level,
                    low: level,
                    mid: level,
                }
            })
            .collect::<Vec<_>>();
        let data = HeroWave {
            zoom,
            buckets: &buckets,
            beats: &[],
            cues: &[],
            downbeats: &[],
            unready: &[],
            loop_region: None,
            position: 0.5,
        };
        let window = window_bounds(data.position, zoom);
        let mut list = DrawListBuilder::default();
        draw_bars(
            &mut list,
            bounds,
            data,
            &window,
            builtin::skin().wave,
            palette(),
        );
        let mut edges = list
            .finish()
            .commands()
            .iter()
            .filter_map(|command| match command {
                DrawCmd::Fill {
                    geom: Geom::Rect(rect),
                    ..
                } => Some(rect.x),
                _ => None,
            })
            .collect::<Vec<_>>();
        edges.dedup();
        edges
    }

    /// The comb is a grid, so every column sits the same distance from the one
    /// before it. A pitch taken from the box rather than the skin drifts by a
    /// fraction of a pixel per column and swallows a whole gap every so often,
    /// which reads as a black stripe repeating across the waveform.
    #[kithara::test]
    fn the_hero_comb_keeps_one_pitch_across_the_box() {
        for width in [400.0, 517.0, 663.0] {
            let bounds = Rect {
                h: 96.0,
                w: width,
                x: 0.0,
                y: 0.0,
            };
            let edges = column_edges(bounds, DEFAULT_ZOOM);
            let pitches = edges
                .windows(2)
                .map(|pair| pair[1] - pair[0])
                .collect::<Vec<_>>();

            assert!(
                pitches.windows(2).all(|pair| pair[0] == pair[1]),
                "a box {width} wide drew columns at uneven pitches {pitches:?}"
            );
        }
    }
}

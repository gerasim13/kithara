use crate::{
    draw::{DrawListBuilder, Rect, Rgba},
    render::WaveBucket,
    skin::WaveSkin,
};

/// Column pitch: one bar plus the gap after it.
pub(crate) fn step(metrics: WaveSkin) -> f32 {
    metrics.bar_width + metrics.bar_gap
}

#[derive(Clone, Copy)]
pub(crate) struct Played {
    end_x: f32,
    overlay: Rgba,
}

impl Played {
    pub(crate) const fn new(end_x: f32, alpha: f32, color: Rgba) -> Self {
        Self {
            end_x,
            overlay: Rgba { a: alpha, ..color },
        }
    }

    pub(crate) fn colors(self, center_x: f32, colors: [Rgba; 3]) -> [Rgba; 3] {
        if center_x >= self.end_x {
            return colors;
        }
        colors.map(|color| composite(self.overlay, color))
    }
}

fn composite(over: Rgba, under: Rgba) -> Rgba {
    let over_alpha = over.a.clamp(0.0, 1.0);
    let under_alpha = under.a.clamp(0.0, 1.0);
    let under_scale = under_alpha * (1.0 - over_alpha);
    let alpha = over_alpha + under_scale;
    if alpha <= 0.0 {
        return Rgba {
            a: 0.0,
            b: 0.0,
            g: 0.0,
            r: 0.0,
        };
    }
    Rgba {
        a: alpha,
        b: (over.b * over_alpha + under.b * under_scale) / alpha,
        g: (over.g * over_alpha + under.g * under_scale) / alpha,
        r: (over.r * over_alpha + under.r * under_scale) / alpha,
    }
}

/// One column of the waveform: the three bands share a width and nest by
/// level, each drawn from the vertical centre over the previous one.
pub(crate) fn draw_column(
    list: &mut DrawListBuilder,
    bounds: Rect,
    center_x: f32,
    bucket: WaveBucket,
    available_height: f32,
    metrics: WaveSkin,
    colors: [Rgba; 3],
) {
    for (level, color) in [bucket.low, bucket.mid, bucket.high]
        .into_iter()
        .zip(colors)
    {
        let height = level.clamp(0.0, 1.0) * available_height;
        if height <= 0.0 {
            continue;
        }
        list.fill_rect(
            Rect {
                h: height,
                w: metrics.bar_width,
                x: center_x - metrics.bar_width / 2.0,
                y: bounds.y + (bounds.h - height) / 2.0,
            },
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn played_colors_are_resolved_before_a_backend_sees_them() {
        let overlay = Rgba {
            a: 1.0,
            b: 0.2,
            g: 0.1,
            r: 0.0,
        };
        let ink = Rgba {
            a: 1.0,
            b: 1.0,
            g: 0.8,
            r: 0.6,
        };
        let played = Played::new(20.0, 0.75, overlay);

        assert_eq!(played.colors(20.0, [ink; 3]), [ink; 3]);
        assert_eq!(
            played.colors(19.0, [ink; 3]),
            [Rgba {
                a: 1.0,
                b: 0.4,
                g: 0.275,
                r: 0.15,
            }; 3]
        );
    }
}

use super::{DrawCmd, Geom, Path, Pt, Rect, Rgba, Transform};
use crate::text::GlyphRun;

/// An ordered retained list of drawing commands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawList(Vec<DrawCmd>);

impl DrawList {
    /// Returns the commands in drawing order.
    #[must_use]
    pub fn commands(&self) -> &[DrawCmd] {
        &self.0
    }
}

/// Builds a [`DrawList`] in drawing order.
#[derive(Default)]
pub struct DrawListBuilder {
    commands: Vec<DrawCmd>,
}

impl DrawListBuilder {
    /// Adds a nested list scoped to a rectangular clip region.
    pub fn clip(&mut self, region: Rect, list: DrawList) {
        self.commands.push(DrawCmd::Clip { region, list });
    }

    pub fn fill_circle(&mut self, center: Pt, radius: f32, color: Rgba) {
        self.commands.push(DrawCmd::Fill {
            geom: Geom::Circle { center, radius },
            color,
        });
    }

    pub fn stroke_circle(&mut self, center: Pt, radius: f32, color: Rgba, width: f32) {
        self.commands.push(DrawCmd::Stroke {
            geom: Geom::Circle { center, radius },
            color,
            width,
        });
    }

    /// Strokes an arc whose angles are expressed in radians.
    pub fn stroke_arc(
        &mut self,
        center: Pt,
        radius: f32,
        start: f32,
        end: f32,
        color: Rgba,
        width: f32,
    ) {
        self.commands.push(DrawCmd::Stroke {
            geom: Geom::Arc {
                center,
                radius,
                start,
                end,
            },
            color,
            width,
        });
    }

    pub fn stroke_line(&mut self, from: Pt, to: Pt, color: Rgba, width: f32) {
        self.commands.push(DrawCmd::Stroke {
            geom: Geom::Line { from, to },
            color,
            width,
        });
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Rgba) {
        self.commands.push(DrawCmd::Fill {
            geom: Geom::Rect(rect),
            color,
        });
    }

    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Rgba) {
        self.commands.push(DrawCmd::Fill {
            geom: if radius == 0.0 {
                Geom::Rect(rect)
            } else {
                Geom::RoundedRect { rect, radius }
            },
            color,
        });
    }

    pub fn stroke_rounded_rect(&mut self, rect: Rect, radius: f32, color: Rgba, width: f32) {
        self.commands.push(DrawCmd::Stroke {
            geom: if radius == 0.0 {
                Geom::Rect(rect)
            } else {
                Geom::RoundedRect { rect, radius }
            },
            color,
            width,
        });
    }

    pub fn fill_path(&mut self, path: Path, color: Rgba) {
        self.commands.push(DrawCmd::Fill {
            geom: Geom::Path(path),
            color,
        });
    }

    pub fn text(&mut self, run: &GlyphRun, content: &str, transform: Transform, color: Rgba) {
        self.commands.push(DrawCmd::Text {
            run: run.clone(),
            content: content.to_owned(),
            transform,
            color,
        });
    }

    /// Finishes the retained list.
    #[must_use]
    pub fn finish(self) -> DrawList {
        DrawList(self.commands)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn a_zero_radius_rounded_fill_is_the_existing_rect_list() {
        let rect = Rect {
            h: 12.0,
            w: 24.0,
            x: 3.0,
            y: 6.0,
        };
        let color = Rgba {
            a: 1.0,
            b: 0.25,
            g: 0.5,
            r: 0.75,
        };
        let mut expected = DrawListBuilder::default();
        expected.fill_rect(rect, color);
        let mut rounded = DrawListBuilder::default();
        rounded.fill_rounded_rect(rect, 0.0, color);

        assert_eq!(rounded.finish(), expected.finish());
    }

    #[kithara::test]
    fn rounded_strokes_retain_native_geometry_and_canonicalize_zero() {
        let rect = Rect {
            h: 12.0,
            w: 24.0,
            x: 3.0,
            y: 6.0,
        };
        let color = Rgba {
            a: 1.0,
            b: 0.25,
            g: 0.5,
            r: 0.75,
        };
        let mut builder = DrawListBuilder::default();
        builder.stroke_rounded_rect(rect, 4.0, color, 1.5);
        builder.stroke_rounded_rect(rect, 0.0, color, 1.5);

        assert_eq!(
            builder.finish().commands(),
            [
                DrawCmd::Stroke {
                    geom: Geom::RoundedRect { rect, radius: 4.0 },
                    color,
                    width: 1.5,
                },
                DrawCmd::Stroke {
                    geom: Geom::Rect(rect),
                    color,
                    width: 1.5,
                },
            ]
        );
    }

    #[kithara::test]
    fn a_clip_retains_its_region_and_nested_list() {
        let region = Rect {
            h: 20.0,
            w: 40.0,
            x: 3.0,
            y: 6.0,
        };
        let color = Rgba {
            a: 1.0,
            b: 0.25,
            g: 0.5,
            r: 0.75,
        };
        let mut nested = DrawListBuilder::default();
        nested.fill_rect(
            Rect {
                h: 40.0,
                w: 80.0,
                x: -10.0,
                y: -20.0,
            },
            color,
        );
        let nested = nested.finish();
        let mut builder = DrawListBuilder::default();

        builder.clip(region, nested.clone());

        assert_eq!(
            builder.finish().commands(),
            [DrawCmd::Clip {
                region,
                list: nested,
            }]
        );
    }
}

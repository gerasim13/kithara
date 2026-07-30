use super::{DrawCmd, Geom, Pt, Rect, Rgba, Transform};
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

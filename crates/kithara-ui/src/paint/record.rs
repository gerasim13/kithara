use super::{Painter, Pt, Rgba, Transform};
use crate::text::GlyphRun;

#[derive(Debug, PartialEq)]
pub(crate) enum Cmd {
    FillCircle {
        center: Pt,
        radius: f32,
        color: Rgba,
    },
    StrokeCircle {
        center: Pt,
        radius: f32,
        color: Rgba,
        width: f32,
    },
    StrokeArc {
        center: Pt,
        radius: f32,
        start: f32,
        end: f32,
        color: Rgba,
        width: f32,
    },
    StrokeLine {
        from: Pt,
        to: Pt,
        color: Rgba,
        width: f32,
    },
    Text {
        content: String,
        run: GlyphRun,
        transform: Transform,
        color: Rgba,
    },
}

#[derive(Default)]
pub(crate) struct RecordPainter {
    commands: Vec<Cmd>,
}

impl RecordPainter {
    pub(crate) fn finish(self) -> Vec<Cmd> {
        self.commands
    }
}

impl Painter for RecordPainter {
    fn fill_circle(&mut self, center: Pt, radius: f32, color: Rgba) {
        self.commands.push(Cmd::FillCircle {
            center,
            radius,
            color,
        });
    }

    fn stroke_arc(
        &mut self,
        center: Pt,
        radius: f32,
        start: f32,
        end: f32,
        color: Rgba,
        width: f32,
    ) {
        self.commands.push(Cmd::StrokeArc {
            center,
            radius,
            start,
            end,
            color,
            width,
        });
    }

    fn stroke_circle(&mut self, center: Pt, radius: f32, color: Rgba, width: f32) {
        self.commands.push(Cmd::StrokeCircle {
            center,
            radius,
            color,
            width,
        });
    }

    fn stroke_line(&mut self, from: Pt, to: Pt, color: Rgba, width: f32) {
        self.commands.push(Cmd::StrokeLine {
            from,
            to,
            color,
            width,
        });
    }

    fn text(&mut self, run: &GlyphRun, content: &str, transform: Transform, color: Rgba) {
        self.commands.push(Cmd::Text {
            content: content.to_owned(),
            run: run.clone(),
            transform,
            color,
        });
    }
}

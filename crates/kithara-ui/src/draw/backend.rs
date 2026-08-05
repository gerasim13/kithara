use super::{DrawCmd, DrawList, Geom, Rect, Rgba, Transform};
use crate::text::GlyphRun;

/// Consumes toolkit-neutral retained drawing commands.
pub trait Backend {
    /// Encodes a nested list inside a rectangular clip region.
    fn clip(&mut self, region: Rect, list: &DrawList);

    fn fill(&mut self, geom: &Geom, color: Rgba);

    fn stroke(&mut self, geom: &Geom, color: Rgba, width: f32);

    fn text(&mut self, run: &GlyphRun, content: &str, transform: Transform, color: Rgba);
}

/// Replays a retained list into a rendering backend.
pub fn replay<B: Backend>(list: &DrawList, backend: &mut B) {
    for command in list.commands() {
        match command {
            DrawCmd::Clip { region, list } => backend.clip(*region, list),
            DrawCmd::Fill { geom, color } => backend.fill(geom, *color),
            DrawCmd::Stroke { geom, color, width } => backend.stroke(geom, *color, *width),
            DrawCmd::Text {
                run,
                content,
                transform,
                color,
            } => backend.text(run, content, *transform, *color),
        }
    }
}

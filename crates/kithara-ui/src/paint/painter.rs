use crate::text::GlyphRun;

/// A toolkit-neutral RGBA colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub a: f32,
    pub b: f32,
    pub g: f32,
    pub r: f32,
}

/// A toolkit-neutral point in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

/// A toolkit-neutral rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub h: f32,
    pub w: f32,
    pub x: f32,
    pub y: f32,
}

/// A toolkit-neutral affine transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub xx: f32,
    pub xy: f32,
    pub yx: f32,
    pub yy: f32,
    pub dx: f32,
    pub dy: f32,
}

impl Transform {
    /// The identity transform.
    pub const IDENTITY: Self = Self {
        xx: 1.0,
        xy: 0.0,
        yx: 0.0,
        yy: 1.0,
        dx: 0.0,
        dy: 0.0,
    };

    #[cfg(feature = "render")]
    pub(crate) const fn apply(self, point: Pt) -> Pt {
        Pt {
            x: self.xx * point.x + self.xy * point.y + self.dx,
            y: self.yx * point.x + self.yy * point.y + self.dy,
        }
    }

    /// Creates a translation transform.
    #[must_use]
    pub const fn translate(offset: Pt) -> Self {
        Self {
            dx: offset.x,
            dy: offset.y,
            ..Self::IDENTITY
        }
    }
}

/// Minimal drawing port implemented by rendering backends.
pub trait Painter {
    fn fill_circle(&mut self, center: Pt, radius: f32, color: Rgba);

    /// Strokes an arc whose angles are expressed in radians.
    fn stroke_arc(
        &mut self,
        center: Pt,
        radius: f32,
        start: f32,
        end: f32,
        color: Rgba,
        width: f32,
    );

    fn stroke_circle(&mut self, center: Pt, radius: f32, color: Rgba, width: f32);

    fn stroke_line(&mut self, from: Pt, to: Pt, color: Rgba, width: f32);

    fn text(&mut self, run: &GlyphRun, content: &str, transform: Transform, color: Rgba);
}

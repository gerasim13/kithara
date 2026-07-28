#[cfg(feature = "render")]
pub(crate) mod canvas;
#[cfg(feature = "render")]
mod painter;
#[cfg(all(test, feature = "render"))]
pub(crate) mod record;

#[cfg(feature = "render")]
pub(crate) use painter::{Painter, Pt, Rect, Rgba, TextStyle};

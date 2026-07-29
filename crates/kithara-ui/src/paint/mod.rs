#[cfg(feature = "render")]
pub(crate) mod canvas;
#[cfg(any(feature = "render", feature = "vello-backend"))]
mod painter;
#[cfg(all(test, feature = "render"))]
pub(crate) mod record;
#[cfg(feature = "vello-backend")]
pub mod scene;

#[cfg(any(feature = "render", feature = "vello-backend"))]
pub use painter::{Painter, Pt, Rect, Rgba, Transform};

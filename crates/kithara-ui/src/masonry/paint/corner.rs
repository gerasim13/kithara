use crate::{
    atoms::design::corner::{corner_ring, corner_verbs, frame_clip},
    draw::{DrawListBuilder, FillRule, Path, Rect, Rgba},
    layout::{FrameCorners, FrameSides},
};

/// A box with `radius` taken off the corners `corners` names and the rest left
/// square.
///
/// The window has no outline of its own: what stands at its corner is a module,
/// and this is the shape that module fills to give the window one.
pub(crate) fn corner_path(bounds: Rect, radius: f32, corners: FrameCorners) -> Path {
    Path::new(FillRule::NonZero, corner_verbs(bounds, radius, corners))
}

/// Fills the frame of a rounded box as a ring: the box's outline with the
/// outline of what it encloses cut out of it.
///
/// The frame is filled rather than stroked because that is what a square frame
/// already is - a band inside the box it frames - and a rounded corner must not
/// change where the band lies. A side the layout leaves out is trimmed by the
/// clip, which is exact: a side is only ever left out where the module meets
/// another one, and a corner there is square.
pub(crate) fn corner_frame(
    list: &mut DrawListBuilder,
    bounds: Rect,
    radius: f32,
    corners: FrameCorners,
    sides: FrameSides,
    color: Rgba,
    width: f32,
) {
    if width <= 0.0 {
        return;
    }
    let mut band = list.child();
    band.fill_path(corner_ring(bounds, radius, corners, width), color);
    list.clip(frame_clip(bounds, sides, width), band.finish());
}

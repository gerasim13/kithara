use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A table of tracks, with columns the document declares.
pub(crate) struct TrackList;

impl Control for TrackList {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.track_list.size
    }
}

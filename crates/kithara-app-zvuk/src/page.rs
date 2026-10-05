use crate::Track;

/// One catalogue page and the exact result total established by the service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrackPage {
    /// Tracks loaded in this page, in the service's order.
    pub(crate) tracks: Vec<Track>,
    /// Server count or terminal page length; unknown when neither is supplied.
    pub(crate) total: Option<usize>,
}

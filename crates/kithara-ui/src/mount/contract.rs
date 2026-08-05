use crate::{size::SizeSpec, skin::SkinDoc};

/// One built-in control, described in the one file that owns it.
///
/// This half of the contract is what the document alone settles, so it is also
/// the half that still compiles when no renderer is compiled in.
pub(crate) trait Control {
    /// The size the skin gives this control, before any override the document
    /// declares on the node itself.
    fn size(&self, skin: &SkinDoc) -> SizeSpec;
}

/// One operation, applied to whichever control the document named.
///
/// [`super::dispatch`] holds the single match over `ControlSpec`; a visitor is
/// how a caller rides through it instead of writing that match again. It stays
/// generic rather than boxing because a retained node is generic over the
/// host's action type, which a trait object could not carry.
pub(crate) trait Visit {
    type Output;

    fn visit<C: Control>(self, control: &C) -> Self::Output;
}

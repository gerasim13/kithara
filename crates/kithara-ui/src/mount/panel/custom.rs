use crate::{ids::InternId, mount::Control, size::SizeSpec, skin::SkinDoc};

/// Content the application registered under a kind, standing in the box the
/// document declares for it.
///
/// The toolkit knows the name and nothing else. `Fill` is what the box says
/// when the document does not narrow it, and a widget whose intrinsic extent
/// matters says so by declaring `Shrink` on that axis instead.
pub(crate) struct Custom {
    pub(crate) kind: InternId,
}

impl Custom {
    pub(crate) const fn new(kind: InternId) -> Self {
        Self { kind }
    }
}

impl Control for Custom {
    fn size(&self, _skin: &SkinDoc) -> SizeSpec {
        SizeSpec::FILL
    }
}

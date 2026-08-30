use crate::{draw::Pt, expand::Binding, ids::InternId};

/// One placement of a stage, as its host mounts it.
///
/// The point is where the child's box goes inside the stage, not an offset on
/// what it draws, so the region that answers the pointer travels with it. A
/// placement with somewhere to write may be carried; one without stands where
/// the document puts it.
#[non_exhaustive]
pub struct PlacedMount<'a> {
    pub path: InternId,
    pub at: Pt,
    /// Where the point comes from, for a host that re-reads endpoints into a
    /// tree it keeps rather than mounting the document again.
    pub read: Option<&'a Binding>,
    pub write: Option<&'a Binding>,
    pub snap: Option<Snap>,
}

/// What takes a carried placement: the points of the placements its magnet
/// names, and how near it must come before one of them does.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Snap {
    pub to: Vec<Pt>,
    pub within: f32,
}

impl Snap {
    /// Where a drag ends: the nearest point in reach, or where the pointer left
    /// it. Both hosts publish through this, so a magnet answers once.
    #[must_use]
    pub fn take(&self, at: Pt) -> Pt {
        self.to
            .iter()
            .copied()
            .filter(|target| target.distance(at) <= self.within)
            .min_by(|one, other| one.distance(at).total_cmp(&other.distance(at)))
            .unwrap_or(at)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{Pt, Snap};

    fn snap(to: Vec<Pt>, within: f32) -> Snap {
        Snap { to, within }
    }

    /// Two targets in reach are not a tie: the nearer one takes the drag.
    #[kithara::test]
    fn the_nearest_target_in_reach_takes_the_point() {
        let snap = snap(vec![Pt { x: 0.0, y: 0.0 }, Pt { x: 40.0, y: 0.0 }], 64.0);

        assert_eq!(snap.take(Pt { x: 30.0, y: 0.0 }), Pt { x: 40.0, y: 0.0 });
    }

    /// A target farther than the reach leaves the point where the drag ended.
    #[kithara::test]
    fn a_target_out_of_reach_leaves_the_point() {
        let snap = snap(vec![Pt { x: 0.0, y: 0.0 }], 16.0);
        let at = Pt { x: 100.0, y: 0.0 };

        assert_eq!(snap.take(at), at);
    }

    /// The reach is met at its own value, not only under it.
    #[kithara::test]
    fn a_target_at_the_reach_still_takes_the_point() {
        let snap = snap(vec![Pt { x: 0.0, y: 0.0 }], 16.0);

        assert_eq!(snap.take(Pt { x: 16.0, y: 0.0 }), Pt { x: 0.0, y: 0.0 });
    }

    /// A magnet naming nothing that stands leaves every drag alone.
    #[kithara::test]
    fn a_magnet_without_targets_leaves_the_point() {
        let snap = snap(Vec::new(), 64.0);
        let at = Pt { x: 7.0, y: 9.0 };

        assert_eq!(snap.take(at), at);
    }
}

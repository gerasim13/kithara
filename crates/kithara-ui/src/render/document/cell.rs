use crate::{
    module::MeasureAxis,
    size::{SizeSpec, stands},
};

/// The band of room a cell stands in: from this much room on the measured
/// axis, and until that much. An open ceiling means it never goes away again.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Band {
    pub from: f32,
    pub until: Option<f32>,
}

impl Band {
    /// A cell that names no band stands in every room its flow is given.
    pub const ALWAYS: Self = Self {
        from: 0.0,
        until: None,
    };

    #[must_use]
    pub const fn new(from: f32, until: Option<f32>) -> Self {
        Self { from, until }
    }

    /// Whether this cell stands in the room its flow turned out to have.
    #[must_use]
    pub fn stands(self, room: f32) -> bool {
        stands(self.from, self.until, room)
    }
}

/// One cell of a split, as its host mounts it.
#[non_exhaustive]
pub struct SplitMount<T> {
    /// The room this cell stands in.
    pub band: Band,
    /// Its share of the room among the cells standing beside it.
    pub weight: f32,
    /// The box it composes to.
    pub size: SizeSpec,
    pub output: T,
}

/// One child of a row or column, as its host mounts it.
#[non_exhaustive]
pub struct GroupMount<T> {
    /// The room this child stands in.
    pub band: Band,
    /// What it needs on the flow's own axis, when it names a floor.
    pub minimum: Option<f32>,
    pub output: T,
}

/// Branches whose choice belongs to the layout pass.
///
/// The document names a threshold per branch on one axis, and only the pass
/// that knows the room can say which branch stands. Every branch is mounted;
/// `steps` is one shorter than the branches, because the first stands below
/// the first threshold.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Measured {
    /// The axis whose room decides.
    pub axis: MeasureAxis,
    /// The room each branch after the first stands from, in document order.
    pub steps: Vec<f32>,
    /// The box the node itself asks for.
    pub size: SizeSpec,
}

impl Measured {
    /// Which branch stands in this much room, as an index into the branches.
    #[must_use]
    pub fn branch(&self, room: f32) -> usize {
        self.steps
            .iter()
            .rposition(|from| *from <= room)
            .map_or(0, |index| index + 1)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{Band, Measured};
    use crate::{module::MeasureAxis, size::SizeSpec};

    fn measured() -> Measured {
        Measured {
            axis: MeasureAxis::Width,
            steps: vec![100.0, 200.0],
            size: SizeSpec::FILL,
        }
    }

    /// A threshold is reached at its own value, not past it.
    #[kithara::test]
    fn a_branch_stands_from_the_room_it_names() {
        let measured = measured();

        assert_eq!(measured.branch(99.0), 0);
        assert_eq!(measured.branch(100.0), 1);
        assert_eq!(measured.branch(199.0), 1);
        assert_eq!(measured.branch(200.0), 2);
    }

    /// An axis nobody bounded takes the last branch, which is the widest one.
    #[kithara::test]
    fn an_unbounded_axis_takes_the_last_branch() {
        assert_eq!(measured().branch(f32::INFINITY), 2);
    }

    /// A cell that names no band never goes away.
    #[kithara::test]
    fn an_unnamed_band_stands_in_every_room() {
        assert!(Band::ALWAYS.stands(0.0));
        assert!(Band::ALWAYS.stands(f32::INFINITY));
    }

    /// A band starts at the room it names and stops below its ceiling.
    #[kithara::test]
    fn a_band_starts_at_its_floor() {
        let band = Band::new(100.0, Some(200.0));

        assert!(!band.stands(99.0));
        assert!(band.stands(100.0));
    }

    #[kithara::test]
    fn a_band_stops_below_its_ceiling() {
        let band = Band::new(100.0, Some(200.0));

        assert!(band.stands(199.0));
        assert!(!band.stands(200.0));
    }
}

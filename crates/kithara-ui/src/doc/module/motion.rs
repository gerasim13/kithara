use serde::{Deserialize, Serialize};

use crate::geom::{Pt, Transform};

/// How an object is offset from the box its container placed it in.
///
/// Composed as `translate(position)` then `rotate(rotation)` then
/// `scale(scale)` then `translate(-anchor)`, read right to left the way a point
/// travels through it, so a rotation turns about the object's own anchor rather
/// than about the corner of its box. The anchor is measured from that corner,
/// and a pose left at its defaults leaves the object exactly where the
/// container put it.
///
/// The offset reaches the picture and nothing else: not the layout the
/// container computed, and not the region that answers the pointer.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Pose {
    /// Where the anchor lands, in the box the container placed.
    pub position: (f32, f32),
    /// The point of the object that `position` places, from the box's corner.
    pub anchor: (f32, f32),
    /// Scale factors, `1.0` being unscaled.
    pub scale: (f32, f32),
    /// Clockwise rotation in degrees, because y grows downward on a screen.
    pub rotation: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            position: (0.0, 0.0),
            anchor: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
        }
    }
}

impl Pose {
    /// Whether this leaves its object where the container put it, so a caller
    /// can skip the work entirely and — more importantly — so a document with
    /// no objects in it draws exactly the list it drew before.
    #[must_use]
    pub fn is_still(&self) -> bool {
        *self == Self::default()
    }

    /// Whether this does anything but move its object.
    ///
    /// A move applies to a whole subtree, because every box in it shifts by the
    /// same vector. A turn or a scale does not: each box would turn about its
    /// own corner.
    #[must_use]
    pub fn turns(&self) -> bool {
        self.rotation != 0.0 || self.scale != (1.0, 1.0)
    }

    /// This pose a fraction of the way toward `to`.
    ///
    /// `phase` is clamped to `0.0..=1.0`, so a model that overshoots settles at
    /// `to` rather than flying past it. Every field travels, the rotation in
    /// degrees, which is what makes `0.0` to `360.0` one full turn rather than
    /// no turn at all.
    #[must_use]
    pub fn between(&self, to: &Self, phase: f32) -> Self {
        let phase = phase.clamp(0.0, 1.0);
        let travel = |from: f32, to: f32| (to - from).mul_add(phase, from);
        let pair = |from: (f32, f32), to: (f32, f32)| (travel(from.0, to.0), travel(from.1, to.1));
        Self {
            position: pair(self.position, to.position),
            anchor: pair(self.anchor, to.anchor),
            scale: pair(self.scale, to.scale),
            rotation: travel(self.rotation, to.rotation),
        }
    }

    /// This pose as one transform, in the coordinates the box was handed in.
    #[must_use]
    pub fn matrix(&self) -> Transform {
        Transform::translate(Pt {
            x: -self.anchor.0,
            y: -self.anchor.1,
        })
        .then(Transform::scale(Pt {
            x: self.scale.0,
            y: self.scale.1,
        }))
        .then(Transform::rotate(self.rotation.to_radians()))
        .then(Transform::translate(Pt {
            x: self.position.0 + self.anchor.0,
            y: self.position.1 + self.anchor.1,
        }))
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::Pose;
    use crate::geom::Pt;

    #[kithara::test]
    fn a_pose_left_alone_moves_nothing() {
        assert!(Pose::default().is_still());
    }

    #[kithara::test]
    fn a_scale_of_one_is_the_unscaled_default() {
        assert_eq!(Pose::default().scale, (1.0, 1.0));
    }

    /// The anchor is the point that stays put, so a turn about it leaves it
    /// exactly where it was rather than swinging it about the box corner.
    #[kithara::test]
    fn a_turn_leaves_the_anchor_where_it_was() {
        let turned = Pose {
            anchor: (20.0, 10.0),
            rotation: 90.0,
            ..Pose::default()
        };

        let held = turned.matrix().apply(Pt { x: 20.0, y: 10.0 });

        assert_eq!(
            Pt {
                x: held.x.round(),
                y: held.y.round(),
            },
            Pt { x: 20.0, y: 10.0 }
        );
    }

    #[kithara::test]
    fn the_start_of_a_track_is_the_pose_it_started_from() {
        let from = Pose::default();
        let to = Pose {
            rotation: 360.0,
            ..Pose::default()
        };

        assert_eq!(from.between(&to, 0.0), from);
    }

    #[kithara::test]
    fn the_end_of_a_track_is_the_pose_it_travelled_to() {
        let from = Pose::default();
        let to = Pose {
            rotation: 360.0,
            ..Pose::default()
        };

        assert_eq!(from.between(&to, 1.0), to);
    }

    #[kithara::test]
    fn halfway_along_a_full_turn_is_half_a_turn() {
        let from = Pose::default();
        let to = Pose {
            rotation: 360.0,
            ..Pose::default()
        };

        assert_eq!(from.between(&to, 0.5).rotation, 180.0);
    }

    /// A model that runs past the end settles at the end, rather than carrying
    /// the object off the page.
    #[kithara::test]
    fn a_phase_past_the_end_settles_at_the_end() {
        let from = Pose::default();
        let to = Pose {
            position: (100.0, 0.0),
            ..Pose::default()
        };

        assert_eq!(from.between(&to, 4.0), to);
    }

    #[kithara::test]
    fn a_still_pose_is_the_identity() {
        assert!(Pose::default().matrix().is_identity());
    }

    #[kithara::test]
    fn a_turned_pose_is_not_still() {
        let turned = Pose {
            rotation: 90.0,
            ..Pose::default()
        };

        assert!(!turned.is_still());
    }
}

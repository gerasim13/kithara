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

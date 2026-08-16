//! Geometry under a transform.
//!
//! A named shape survives a transform only while the transform leaves it
//! nameable: a rectangle stays a rectangle while the axes stay where they are,
//! a circle stays a circle only under a rotation and one scale. Everything else
//! becomes an outline here, in the neutral list, so that both hosts replay the
//! same points instead of each asking its own toolkit to turn a rectangle.

use kurbo::{Arc, Circle, PathEl, Point, RoundedRect, Shape};
use num_traits::cast::AsPrimitive;

use super::{
    ir::{Pt, Rect, Transform},
    path::Verb,
};

/// How closely a flattened curve follows the one it replaces, in logical
/// pixels. A tenth of a pixel is under what either rasteriser resolves.
const TOLERANCE: f64 = 0.1;

/// The upright rectangle that holds this one after the transform.
///
/// Exact while the transform keeps the axes — which is the only case a caller
/// is allowed to keep a `Rect` for. Where a rectangle must stay a rectangle
/// even though the transform turned it, this is the box it fits in: a clip
/// region and an image destination have nowhere else to go, because neither
/// the neutral command nor either toolkit's image carries a matrix.
pub(super) fn bounds(rect: Rect, by: Transform) -> Rect {
    let corners = [
        by.apply(Pt {
            x: rect.x,
            y: rect.y,
        }),
        by.apply(Pt {
            x: rect.x + rect.w,
            y: rect.y,
        }),
        by.apply(Pt {
            x: rect.x + rect.w,
            y: rect.y + rect.h,
        }),
        by.apply(Pt {
            x: rect.x,
            y: rect.y + rect.h,
        }),
    ];
    let fold = |pick: fn(f32, f32) -> f32, of: fn(Pt) -> f32| {
        corners
            .iter()
            .map(|corner| of(*corner))
            .fold(of(corners[0]), pick)
    };
    let (left, right) = (fold(f32::min, |at| at.x), fold(f32::max, |at| at.x));
    let (top, bottom) = (fold(f32::min, |at| at.y), fold(f32::max, |at| at.y));
    Rect {
        h: bottom - top,
        w: right - left,
        x: left,
        y: top,
    }
}

pub(super) fn rect_verbs(shape: Rect, by: Transform) -> Vec<Verb> {
    let corner = |x: f32, y: f32| Verb::LineTo(by.apply(Pt { x, y }));
    vec![
        Verb::MoveTo(by.apply(Pt {
            x: shape.x,
            y: shape.y,
        })),
        corner(shape.x + shape.w, shape.y),
        corner(shape.x + shape.w, shape.y + shape.h),
        corner(shape.x, shape.y + shape.h),
        Verb::Close,
    ]
}

pub(super) fn rounded_verbs(shape: Rect, radius: f32, by: Transform) -> Vec<Verb> {
    let radius: f64 = radius.as_();
    flatten(
        &RoundedRect::new(
            shape.x.as_(),
            shape.y.as_(),
            (shape.x + shape.w).as_(),
            (shape.y + shape.h).as_(),
            radius,
        ),
        by,
    )
}

pub(super) fn circle_verbs(center: Pt, radius: f32, by: Transform) -> Vec<Verb> {
    flatten(
        &Circle::new(Point::new(center.x.as_(), center.y.as_()), radius.as_()),
        by,
    )
}

pub(super) fn arc_verbs(center: Pt, radius: f32, start: f32, end: f32, by: Transform) -> Vec<Verb> {
    let radius: f64 = radius.as_();
    let start: f64 = start.as_();
    let end: f64 = end.as_();
    flatten(
        &Arc::new(
            Point::new(center.x.as_(), center.y.as_()),
            (radius, radius),
            start,
            end - start,
            0.0,
        ),
        by,
    )
}

pub(super) fn path_verbs<'a, Verbs>(verbs: Verbs, by: Transform) -> Vec<Verb>
where
    Verbs: IntoIterator<Item = &'a Verb>,
{
    verbs.into_iter().map(|verb| moved(*verb, by)).collect()
}

fn moved(verb: Verb, by: Transform) -> Verb {
    match verb {
        Verb::Close => Verb::Close,
        Verb::CurveTo { first, second, to } => Verb::CurveTo {
            first: by.apply(first),
            second: by.apply(second),
            to: by.apply(to),
        },
        Verb::LineTo(to) => Verb::LineTo(by.apply(to)),
        Verb::MoveTo(to) => Verb::MoveTo(by.apply(to)),
        Verb::QuadTo { control, to } => Verb::QuadTo {
            control: by.apply(control),
            to: by.apply(to),
        },
    }
}

fn flatten(shape: &impl Shape, by: Transform) -> Vec<Verb> {
    shape
        .path_elements(TOLERANCE)
        .map(|element| element_verb(element, by))
        .collect()
}

fn element_verb(element: PathEl, by: Transform) -> Verb {
    let at = |point: Point| {
        by.apply(Pt {
            x: point.x.as_(),
            y: point.y.as_(),
        })
    };
    match element {
        PathEl::ClosePath => Verb::Close,
        PathEl::CurveTo(first, second, to) => Verb::CurveTo {
            first: at(first),
            second: at(second),
            to: at(to),
        },
        PathEl::LineTo(to) => Verb::LineTo(at(to)),
        PathEl::MoveTo(to) => Verb::MoveTo(at(to)),
        PathEl::QuadTo(control, to) => Verb::QuadTo {
            control: at(control),
            to: at(to),
        },
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{Transform, bounds, rect_verbs};
    use crate::draw::{Pt, Rect, Verb};

    const BOX: Rect = Rect {
        h: 20.0,
        w: 40.0,
        x: 10.0,
        y: 5.0,
    };

    #[kithara::test]
    fn a_translation_moves_a_rectangle_and_keeps_its_extent() {
        let moved = bounds(BOX, Transform::translate(Pt { x: 3.0, y: -2.0 }));

        assert_eq!(
            moved,
            Rect {
                h: 20.0,
                w: 40.0,
                x: 13.0,
                y: 3.0,
            }
        );
    }

    /// A mirror is a scale by a negative factor, and a rectangle has no
    /// negative extent to hand a backend.
    #[kithara::test]
    fn a_mirrored_rectangle_is_normalised() {
        let mirrored = bounds(BOX, Transform::scale(Pt { x: -1.0, y: 1.0 }));

        assert_eq!(mirrored.x, -50.0);
    }

    #[kithara::test]
    fn a_mirrored_rectangle_keeps_its_width() {
        let mirrored = bounds(BOX, Transform::scale(Pt { x: -1.0, y: 1.0 }));

        assert_eq!(mirrored.w, 40.0);
    }

    #[kithara::test]
    fn a_rectangle_walked_as_an_outline_closes() {
        let verbs = rect_verbs(BOX, Transform::IDENTITY);

        assert_eq!(verbs.last(), Some(&Verb::Close));
    }

    /// A quarter turn sends the near corner `(10, 5)` to `(-5, 10)`. A quarter
    /// turn has no exact `sin_cos` in `f32`, so the corner is read at the pixel
    /// it falls in rather than at its bits; the property being checked is where
    /// the rotation puts it, not how the library rounds.
    #[kithara::test]
    fn a_quarter_turn_puts_the_first_corner_where_the_rotation_sends_it() {
        let verbs = rect_verbs(BOX, Transform::rotate(core::f32::consts::FRAC_PI_2));
        let Some(&Verb::MoveTo(corner)) = verbs.first() else {
            panic!("a rectangle starts by moving to its near corner");
        };

        assert_eq!(
            Pt {
                x: corner.x.round(),
                y: corner.y.round(),
            },
            Pt { x: -5.0, y: 10.0 }
        );
    }
}

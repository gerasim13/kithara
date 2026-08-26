use std::cell::RefCell;

use iced::widget::canvas;

use crate::draw::{CachedValue, DrawList};

/// What a frame cost a control that keeps what it drew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Marked {
    /// The key held: nothing was built and the geometry stands.
    Kept,
    /// The picture was built and came out the same: the geometry stands.
    Same,
    /// The picture came out different: the geometry was dropped.
    Changed,
}

/// One drawn picture kept beside the key it was built from, and the geometry
/// tessellated from that picture.
///
/// The key is what the drawing was made from, compared whole. A hash would be
/// cheaper to carry, but it would have to be taught every field the drawing
/// reads, and a field it was never taught freezes the control on the screen;
/// derived equality cannot forget one. The picture is compared as well, because
/// that answers the other question: a key that moved without changing the
/// drawing must not cost a tessellation.
pub(crate) struct Marks<Key>
where
    Key: PartialEq,
{
    geometry: canvas::Cache,
    kept: RefCell<CachedValue<Option<Key>, DrawList>>,
}

impl<Key> Default for Marks<Key>
where
    Key: PartialEq,
{
    fn default() -> Self {
        Self {
            geometry: canvas::Cache::default(),
            kept: RefCell::default(),
        }
    }
}

impl<Key> Marks<Key>
where
    Key: PartialEq,
{
    /// Builds the picture only when the key it hangs on moved, drops the kept
    /// geometry only when the picture that came out differs, and reports which
    /// of the two it had to do. Those answers are the whole of this cache, so
    /// they are returned rather than inferred from the pixels.
    pub(crate) fn mark(&self, key: Key, build: impl FnOnce() -> DrawList) -> Marked {
        let mut kept = self.kept.borrow_mut();
        if kept.value().is_some() && kept.key().as_ref() == Some(&key) {
            return Marked::Kept;
        }
        let list = build();
        let changed = kept.value() != Some(&list);
        if changed {
            self.geometry.clear();
        }
        kept.update(Some(key), Some(list));
        if changed {
            Marked::Changed
        } else {
            Marked::Same
        }
    }

    /// The kept picture and the geometry drawn from it, for as long as it takes
    /// to replay them. Nothing at all before the first [`Self::mark`].
    pub(crate) fn drawn<T>(&self, with: impl FnOnce(&canvas::Cache, &DrawList) -> T) -> Option<T> {
        let kept = self.kept.borrow();
        kept.value().map(|list| with(&self.geometry, list))
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{Marked, Marks};
    use crate::draw::{DrawList, DrawListBuilder, Rect, Rgba};

    /// One of two pictures a frame apart: the fill is all red, or none of it.
    fn filled(red: f32) -> DrawList {
        let mut builder = DrawListBuilder::default();
        builder.fill_rect(
            Rect {
                h: 10.0,
                w: 20.0,
                x: 0.0,
                y: 0.0,
            },
            Rgba {
                a: 1.0,
                b: 1.0 - red,
                g: 0.0,
                r: red,
            },
        );
        builder.finish()
    }

    #[kithara::test]
    fn the_first_frame_builds_its_picture() {
        let marks = Marks::default();

        assert_eq!(marks.mark(1_u8, || filled(1.0)), Marked::Changed);
    }

    /// The whole point of the key: a control nothing touched must not be drawn
    /// from again, however cheap the drawing would have been.
    #[kithara::test]
    fn a_key_that_held_builds_nothing() {
        let marks = Marks::default();
        marks.mark(1_u8, || filled(1.0));

        assert_eq!(
            marks.mark(1, || panic!("a key that held must not build")),
            Marked::Kept
        );
    }

    /// The second stage answers what the key cannot: a key may move without the
    /// picture moving with it, and tessellating again for the same picture is
    /// the cost the list comparison exists to spare.
    #[kithara::test]
    fn a_key_that_moved_onto_the_same_picture_keeps_it() {
        let marks = Marks::default();
        marks.mark(1_u8, || filled(1.0));

        assert_eq!(marks.mark(2, || filled(1.0)), Marked::Same);
    }

    #[kithara::test]
    fn a_picture_that_moved_is_drawn_again() {
        let marks = Marks::default();
        marks.mark(1_u8, || filled(1.0));

        assert_eq!(marks.mark(2, || filled(0.0)), Marked::Changed);
    }

    /// A key that moved takes its picture with it, or the next frame would
    /// answer for a key it no longer holds.
    #[kithara::test]
    fn a_key_that_moved_is_the_one_the_next_frame_is_asked_about() {
        let marks = Marks::default();
        marks.mark(1_u8, || filled(1.0));
        marks.mark(2, || filled(0.0));

        assert_eq!(
            marks.mark(2, || panic!("the key that came with the picture must hold")),
            Marked::Kept
        );
    }
}

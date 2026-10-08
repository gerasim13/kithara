use super::{Grip, IndexEvent};
use crate::{
    atoms::painter::ControlPainter,
    render::{ReadValue, Skin, document::Ctx},
};

/// A control that draws itself: one painter, and the value it paints.
///
/// Declared once, in the control's own file, so what a control looks like
/// cannot differ between the two hosts by construction. Each host mounts it
/// through its own adapter and adds nothing to the picture.
pub(crate) trait Draws {
    type Painter: ControlPainter;

    /// What it draws this frame, or nothing at all when its endpoint has not
    /// said yet - an unbound switch is an empty box, not an idle switch.
    fn data(&self, read: Reading<'_>) -> Option<<Self::Painter as ControlPainter>::Data>;

    /// What the pointer means to it where the document says the leaf owns input.
    fn grip(&self, _skin: &Skin, _data: &<Self::Painter as ControlPainter>::Data) -> Grip {
        Grip::None
    }

    fn index_event(&self) -> Option<IndexEvent<<Self::Painter as ControlPainter>::Data>> {
        None
    }

    /// The painter, with the skin already resolved into it.
    fn painter(&self, skin: &Skin) -> Self::Painter;
}

/// What a control is handed when it decides what to draw.
///
/// Values are reached through [`Ctx`] and nowhere else, so a control cannot be
/// written that quietly misses what the host answers for itself — the frame's
/// own clock among it.
///
/// The skin is here as well as on [`Draws::painter`] because what a control
/// draws can come from the skin rather than only how it is drawn: a picture is
/// named by the document and carried by the skin.
#[derive(Clone, Copy)]
pub(crate) struct Reading<'a> {
    pub(crate) skin: &'a Skin,
    pub(crate) scope: &'a str,
    pub(crate) ctx: Ctx<'a, 'a>,
    pub(crate) value: Option<&'a ReadValue<'a>>,
}

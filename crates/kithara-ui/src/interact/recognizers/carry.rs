use super::super::{CursorShape, Hit, Input, Outcome, PointerOwnership, PointerPhase};
use crate::draw::Pt;

/// Press-and-move that carries one placement of a scene.
///
/// It answers in the space the hit is expressed in: the press fixes where
/// inside the placement the pointer took hold, and every move afterwards says
/// where that placement's corner has to be for the pointer to stay on the same
/// spot of it. Which corner that is in a scene, and whether a magnet moves it
/// somewhere else, belongs to whoever mounted the placement.
#[derive(Default)]
pub(crate) struct Carry {
    /// Where the press landed, from the placement's own corner.
    grab: Option<Pt>,
}

impl Carry {
    fn corner(&self, hit: &Hit) -> Option<Pt> {
        let grab = self.grab?;
        let at = hit.at()?;
        Some(Pt {
            x: at.x - grab.x,
            y: at.y - grab.y,
        })
    }

    pub(crate) const fn cursor(&self) -> CursorShape {
        if self.grab.is_some() {
            CursorShape::Grabbing
        } else {
            CursorShape::Grab
        }
    }

    /// Whether a pointer is carrying this placement right now.
    pub(crate) const fn is_carried(&self) -> bool {
        self.grab.is_some()
    }

    pub(crate) fn on_input(&mut self, input: Input<'_>, hit: &Hit) -> Outcome<Pt> {
        match input {
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Down => {
                let Some(at) = hit.inside() else {
                    return Outcome::IGNORED;
                };
                let area = hit.area();
                self.grab = Some(Pt {
                    x: at.x - area.x,
                    y: at.y - area.y,
                });
                Outcome::captured().with_ownership(PointerOwnership::Claim)
            }
            Input::Pointer(pointer)
                if matches!(
                    pointer.phase,
                    PointerPhase::Move | PointerPhase::MoveLongPress
                ) =>
            {
                self.corner(hit).map_or(Outcome::IGNORED, Outcome::set)
            }
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Up => {
                let corner = self.corner(hit);
                self.grab = None;
                corner.map_or_else(
                    || Outcome::IGNORED.with_ownership(PointerOwnership::Release),
                    |corner| Outcome::set(corner).with_ownership(PointerOwnership::Release),
                )
            }
            Input::Pointer(pointer)
                if matches!(pointer.phase, PointerPhase::Cancel | PointerPhase::Leave) =>
            {
                self.grab = None;
                Outcome::IGNORED.with_ownership(PointerOwnership::Release)
            }
            Input::InputMethod(_)
            | Input::KeyPressed { .. }
            | Input::KeyReleased { .. }
            | Input::ModifiersChanged(_)
            | Input::Pointer(_)
            | Input::Wheel(_) => Outcome::IGNORED,
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{draw::Rect, interact::mouse as mouse_input};

    /// A placement standing at (100, 50), forty across and forty down.
    fn area() -> Rect {
        Rect {
            h: 40.0,
            w: 40.0,
            x: 100.0,
            y: 50.0,
        }
    }

    fn hit(x: f32, y: f32) -> Hit {
        Hit::new(Some(Pt { x, y }), area())
    }

    fn gone() -> Hit {
        Hit::new(None, area())
    }

    fn pointer(phase: PointerPhase, x: f32, y: f32) -> Input<'static> {
        Input::Pointer(mouse_input(phase, Some(Pt { x, y })))
    }

    #[kithara::test]
    fn a_move_keeps_the_pointer_on_the_spot_it_took_hold_of() {
        let mut carry = Carry::default();

        carry.on_input(pointer(PointerPhase::Down, 110.0, 60.0), &hit(110.0, 60.0));

        assert_eq!(
            carry
                .on_input(pointer(PointerPhase::Move, 150.0, 90.0), &hit(150.0, 90.0))
                .value(),
            Some(Pt { x: 140.0, y: 80.0 }),
            "the corner keeps the ten points the press was in from it"
        );
    }

    #[kithara::test]
    fn a_press_outside_the_placement_carries_nothing() {
        let mut carry = Carry::default();

        carry.on_input(pointer(PointerPhase::Down, 10.0, 10.0), &hit(10.0, 10.0));

        assert!(!carry.is_carried());
    }

    #[kithara::test]
    fn a_move_without_a_press_carries_nothing() {
        let mut carry = Carry::default();

        assert_eq!(
            carry
                .on_input(pointer(PointerPhase::Move, 150.0, 90.0), &hit(150.0, 90.0))
                .value(),
            None
        );
    }

    #[kithara::test]
    fn a_carried_placement_follows_the_pointer_out_of_its_own_box() {
        let mut carry = Carry::default();

        carry.on_input(pointer(PointerPhase::Down, 110.0, 60.0), &hit(110.0, 60.0));

        assert_eq!(
            carry
                .on_input(
                    pointer(PointerPhase::Move, 400.0, 300.0),
                    &hit(400.0, 300.0)
                )
                .value(),
            Some(Pt { x: 390.0, y: 290.0 }),
            "a gesture under way tracks the pointer past the edge it started in"
        );
    }

    #[kithara::test]
    fn a_release_answers_where_it_was_left_and_ends_the_carry() {
        let mut carry = Carry::default();
        carry.on_input(pointer(PointerPhase::Down, 110.0, 60.0), &hit(110.0, 60.0));

        let dropped = carry.on_input(pointer(PointerPhase::Up, 200.0, 100.0), &hit(200.0, 100.0));

        assert_eq!(dropped.value(), Some(Pt { x: 190.0, y: 90.0 }));
        assert!(!carry.is_carried());
    }

    #[kithara::test]
    fn a_release_gives_the_pointer_back() {
        let mut carry = Carry::default();
        carry.on_input(pointer(PointerPhase::Down, 110.0, 60.0), &hit(110.0, 60.0));

        let dropped = carry.on_input(pointer(PointerPhase::Up, 200.0, 100.0), &hit(200.0, 100.0));

        assert_eq!(dropped.ownership(), PointerOwnership::Release);
    }

    #[kithara::test]
    fn a_host_that_reports_no_cursor_moves_nothing() {
        let mut carry = Carry::default();
        carry.on_input(pointer(PointerPhase::Down, 110.0, 60.0), &hit(110.0, 60.0));

        assert_eq!(
            carry
                .on_input(
                    Input::Pointer(mouse_input(PointerPhase::Move, None)),
                    &gone()
                )
                .value(),
            None
        );
    }

    #[kithara::test]
    fn a_cancelled_gesture_drops_what_it_carried() {
        let mut carry = Carry::default();
        carry.on_input(pointer(PointerPhase::Down, 110.0, 60.0), &hit(110.0, 60.0));

        carry.on_input(
            Input::Pointer(mouse_input(PointerPhase::Cancel, None)),
            &gone(),
        );

        assert!(!carry.is_carried());
    }
}

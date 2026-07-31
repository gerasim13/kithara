use super::super::{Hit, Input, Outcome};

/// A press that lands on the control and asks it to act. There is no state and
/// nothing to configure: where the press landed is the whole gesture, so a
/// later event has nothing to change and the cursor rule stays with [`Hover`].
///
/// [`Hover`]: super::super::Hover
pub(crate) fn on_input(input: Input, hit: &Hit) -> Outcome<()> {
    match input {
        Input::PointerDown if hit.over() => Outcome::set(()),
        Input::PointerDown | Input::PointerMoved { .. } | Input::PointerUp | Input::Wheel(_) => {
            Outcome::IGNORED
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{super::super::Scroll, *};
    use crate::draw::{Pt, Rect};

    fn at(x: f32) -> Hit {
        Hit::new(
            Some(Pt { x, y: 13.0 }),
            Rect {
                h: 18.0,
                w: 18.0,
                x: 4.0,
                y: 4.0,
            },
        )
    }

    #[kithara::test]
    fn a_press_on_the_control_acts_and_takes_the_pointer() {
        assert_eq!(on_input(Input::PointerDown, &at(13.0)), Outcome::set(()));
    }

    #[kithara::test]
    fn a_press_beside_the_control_is_left_to_whoever_is_behind() {
        assert_eq!(on_input(Input::PointerDown, &at(40.0)), Outcome::IGNORED);
    }

    #[kithara::test]
    fn nothing_but_a_press_acts() {
        let over = at(13.0);

        for input in [
            Input::PointerMoved {
                at: Pt { x: 13.0, y: 13.0 },
            },
            Input::PointerUp,
            Input::Wheel(Scroll::Lines(1.0)),
        ] {
            assert_eq!(
                on_input(input, &over),
                Outcome::IGNORED,
                "a control that only clicks must not eat the rest of the gesture"
            );
        }
    }
}

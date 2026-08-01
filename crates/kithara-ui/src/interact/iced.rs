use iced::{
    Event, Point, Rectangle,
    mouse::{self, Button, Cursor, ScrollDelta},
};

use super::{CursorShape, Hit, Input, Scroll};
use crate::draw::{Pt, Rect};

pub(crate) fn input(event: &Event) -> Option<Input> {
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) => Some(Input::PointerDown),
        Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Input::PointerMoved {
            at: (*position).into(),
        }),
        Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => Some(Input::PointerUp),
        Event::Mouse(mouse::Event::WheelScrolled { delta }) => Some(Input::Wheel(match delta {
            ScrollDelta::Lines { y, .. } => Scroll::Lines(*y),
            ScrollDelta::Pixels { y, .. } => Scroll::Pixels(*y),
        })),
        _ => None,
    }
}

pub(crate) fn hit(bounds: Rectangle, cursor: Cursor) -> Hit {
    Hit::new(cursor.position().map(Into::into), bounds.into())
}

impl From<Point> for Pt {
    fn from(point: Point) -> Self {
        Self {
            x: point.x,
            y: point.y,
        }
    }
}

impl From<Rectangle> for Rect {
    fn from(rectangle: Rectangle) -> Self {
        Self {
            h: rectangle.height,
            w: rectangle.width,
            x: rectangle.x,
            y: rectangle.y,
        }
    }
}

impl From<CursorShape> for mouse::Interaction {
    fn from(shape: CursorShape) -> Self {
        match shape {
            CursorShape::None => Self::None,
            CursorShape::Grab => Self::Grab,
            CursorShape::Grabbing => Self::Grabbing,
            CursorShape::Pointer => Self::Pointer,
            CursorShape::ResizeH => Self::ResizingHorizontally,
            CursorShape::ResizeV => Self::ResizingVertically,
        }
    }
}

#[cfg(test)]
mod tests {
    use iced::keyboard::{
        self, Location, Modifiers,
        key::{Named, Physical},
    };
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn a_key_carries_no_portable_input() {
        let pressed = Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(Named::Delete),
            modified_key: keyboard::Key::Named(Named::Delete),
            physical_key: Physical::Code(keyboard::key::Code::Delete),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        });

        assert!(
            input(&pressed).is_none(),
            "a key that becomes portable input would be answered by the engine host and never \
             reach the child, which is where the app's unconsumed-key contract lives"
        );
    }

    #[kithara::test]
    fn only_the_left_button_arms_a_gesture() {
        for button in [Button::Right, Button::Middle] {
            assert!(
                input(&Event::Mouse(mouse::Event::ButtonPressed(button))).is_none(),
                "{button:?} must stay with the child"
            );
            assert!(
                input(&Event::Mouse(mouse::Event::ButtonReleased(button))).is_none(),
                "{button:?} must stay with the child"
            );
        }
    }
}

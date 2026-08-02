use iced::{
    Event, Point, Rectangle,
    keyboard::{
        self, Event as KeyboardEvent,
        key::{Key as IcedKey, Named},
    },
    mouse::{self, Button, Cursor, ScrollDelta},
};

use super::{CursorShape, Hit, Input, Key, Modifiers, Scroll};
use crate::draw::{Pt, Rect};

pub(crate) fn input(event: &Event) -> Option<Input<'_>> {
    match event {
        Event::Keyboard(KeyboardEvent::KeyPressed { key, modifiers, .. }) => {
            Some(Input::KeyPressed {
                key: portable_key(key),
                modifiers: portable_modifiers(*modifiers),
            })
        }
        Event::Keyboard(KeyboardEvent::KeyReleased { key, modifiers, .. }) => {
            Some(Input::KeyReleased {
                key: portable_key(key),
                modifiers: portable_modifiers(*modifiers),
            })
        }
        Event::Keyboard(KeyboardEvent::ModifiersChanged(modifiers)) => {
            Some(Input::ModifiersChanged(portable_modifiers(*modifiers)))
        }
        Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) => Some(Input::PointerDown),
        Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Input::PointerMoved {
            at: (*position).into(),
        }),
        Event::Mouse(mouse::Event::CursorLeft) => Some(Input::PointerLeft),
        Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => Some(Input::PointerUp),
        Event::Mouse(mouse::Event::WheelScrolled { delta }) => Some(Input::Wheel(match delta {
            ScrollDelta::Lines { x, y } => Scroll::Lines { x: *x, y: *y },
            ScrollDelta::Pixels { x, y } => Scroll::Pixels { x: *x, y: *y },
        })),
        _ => None,
    }
}

fn portable_key<'a>(key: &'a IcedKey<impl AsRef<str>>) -> Key<'a> {
    match key {
        IcedKey::Named(Named::ArrowDown) => Key::ArrowDown,
        IcedKey::Named(Named::ArrowUp) => Key::ArrowUp,
        IcedKey::Named(Named::Backspace) => Key::Backspace,
        IcedKey::Named(Named::Delete) => Key::Delete,
        IcedKey::Named(Named::Enter) => Key::Enter,
        IcedKey::Named(Named::Escape) => Key::Escape,
        IcedKey::Named(Named::Space) => Key::Space,
        IcedKey::Character(character) => Key::Character(character.as_ref()),
        IcedKey::Named(_) | IcedKey::Unidentified => Key::Other,
    }
}

fn portable_modifiers(modifiers: keyboard::Modifiers) -> Modifiers {
    Modifiers::new(
        modifiers.alt(),
        modifiers.control(),
        modifiers.logo(),
        modifiers.shift(),
    )
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
        self, Location, Modifiers as IcedModifiers,
        key::{Named, Physical},
    };
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn key_press_and_release_preserve_key_and_all_modifiers() {
        let pressed_modifiers = IcedModifiers::ALT | IcedModifiers::LOGO;
        let released_modifiers = IcedModifiers::CTRL | IcedModifiers::SHIFT;
        let pressed = Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character("z".into()),
            modified_key: keyboard::Key::Character("Z".into()),
            physical_key: Physical::Code(keyboard::key::Code::KeyZ),
            location: Location::Standard,
            modifiers: pressed_modifiers,
            text: Some("z".into()),
            repeat: false,
        });
        let released = Event::Keyboard(keyboard::Event::KeyReleased {
            key: keyboard::Key::Named(Named::Delete),
            modified_key: keyboard::Key::Named(Named::Delete),
            physical_key: Physical::Code(keyboard::key::Code::Delete),
            location: Location::Standard,
            modifiers: released_modifiers,
        });

        assert!(matches!(
            input(&pressed),
            Some(Input::KeyPressed {
                key: Key::Character("z"),
                modifiers: decoded,
            }) if decoded == Modifiers::new(true, false, true, false)
        ));
        assert!(matches!(
            input(&released),
            Some(Input::KeyReleased {
                key: Key::Delete,
                modifiers: decoded,
            }) if decoded == Modifiers::new(false, true, false, true)
        ));
    }

    #[kithara::test]
    fn a_modifiers_change_is_portable_input() {
        let changed = Event::Keyboard(keyboard::Event::ModifiersChanged(IcedModifiers::SHIFT));

        let Some(Input::ModifiersChanged(modifiers)) = input(&changed) else {
            panic!("the retained hero wave needs the current modifiers before it sees a press");
        };
        assert!(modifiers.shift());
    }

    #[kithara::test]
    fn cursor_left_decodes_without_inventing_a_position() {
        assert!(matches!(
            input(&Event::Mouse(mouse::Event::CursorLeft)),
            Some(Input::PointerLeft)
        ));
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

    #[kithara::test]
    fn wheel_decode_preserves_both_axes_and_units() {
        for (delta, expected_x, expected_y, pixels) in [
            (ScrollDelta::Lines { x: 3.0, y: -2.0 }, 3.0, -2.0, false),
            (ScrollDelta::Pixels { x: -7.5, y: 4.25 }, -7.5, 4.25, true),
        ] {
            let Some(Input::Wheel(scroll)) =
                input(&Event::Mouse(mouse::Event::WheelScrolled { delta }))
            else {
                panic!("a mouse wheel must become portable input");
            };

            assert_eq!(scroll.x(), expected_x);
            assert_eq!(scroll.y(), expected_y);
            assert_eq!(scroll.is_pixels(), pixels);
        }
    }
}

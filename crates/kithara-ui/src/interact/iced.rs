use iced::{
    Event, Point, Rectangle,
    mouse::{self, Button, Cursor, ScrollDelta},
};

use super::{CursorShape, Hit, Input, Scroll};
use crate::draw::{Pt, Rect};

pub(crate) fn input(event: &Event) -> Option<Input> {
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) => Some(Input::PointerDown),
        Event::Mouse(mouse::Event::CursorMoved { .. }) => Some(Input::PointerMoved),
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
            CursorShape::Pointer => Self::Pointer,
            CursorShape::ResizeH => Self::ResizingHorizontally,
            CursorShape::ResizeV => Self::ResizingVertically,
        }
    }
}

use iced::{
    Event, Rectangle,
    mouse::{self, Button, Cursor, ScrollDelta},
    widget::canvas::Action,
};

use crate::{
    interact::{Hover, iced as iced_interact},
    render::{ControlAction, UiEvent},
};

#[derive(bon::Builder)]
pub(crate) struct ClickActivate {
    path: String,
    hover: Hover,
}

impl ClickActivate {
    pub(crate) fn mouse_interaction(
        &self,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        self.hover
            .cursor(false, &iced_interact::hit(bounds, cursor))
            .into()
    }

    pub(crate) fn update(
        &self,
        _state: &mut (),
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) if cursor.is_over(bounds) => {
                Some(
                    Action::publish(UiEvent::Control {
                        path: self.path.clone(),
                        action: ControlAction::Activate,
                    })
                    .and_capture(),
                )
            }
            _ => None,
        }
    }
}

pub(crate) fn scroll_y(delta: ScrollDelta) -> f32 {
    match delta {
        ScrollDelta::Lines { y, .. } | ScrollDelta::Pixels { y, .. } => y,
    }
}

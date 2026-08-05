use iced::Element;

use crate::{
    module::FaderStyle,
    render::{InputOwner, ReadValue, Skin, UiEvent, fader_slider},
    widgets::Widget,
};

#[derive(bon::Builder)]
pub(crate) struct Fader<'path, 'value, 'data, 'skin> {
    skin: &'skin Skin,
    path: &'path str,
    style: FaderStyle,
    label: Option<&'path str>,
    value: Option<&'value ReadValue<'data>>,
    owner: InputOwner,
}

impl<'a, 'path, 'value, 'data, 'skin> Widget<'a> for Fader<'path, 'value, 'data, 'skin>
where
    'skin: 'a,
{
    fn view(self) -> Element<'a, UiEvent> {
        let Some(ReadValue::Scalar(value)) = self.value else {
            return iced::widget::Space::new().into();
        };
        fader_slider(
            self.path, *value, self.label, self.style, self.skin, self.owner,
        )
    }
}

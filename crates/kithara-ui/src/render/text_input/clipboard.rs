use iced::{
    Event,
    advanced::{Clipboard, clipboard, input_method},
};

use crate::interact::iced as iced_interact;

pub(crate) fn paste(event: &Event, clipboard: &dyn Clipboard) -> Option<Event> {
    if !iced_interact::input(event).is_some_and(|input| input.shortcut('v')) {
        return None;
    }
    clipboard
        .read(clipboard::Kind::Standard)
        .map(|text| Event::InputMethod(input_method::Event::Commit(text)))
}

/// A clipboard holding ` pasted`, for the paste tests of every host.
#[cfg(test)]
pub(crate) struct ClipboardText;

#[cfg(test)]
impl Clipboard for ClipboardText {
    fn read(&self, _kind: clipboard::Kind) -> Option<String> {
        Some(" pasted".to_owned())
    }

    fn write(&mut self, _kind: clipboard::Kind, _contents: String) {}
}

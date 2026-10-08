mod clipboard;
mod paint;
mod program;
mod widget;

#[cfg(test)]
pub(crate) use clipboard::ClipboardText;
pub(crate) use clipboard::paste;
pub(crate) use widget::{search_input, sync_text_input};
